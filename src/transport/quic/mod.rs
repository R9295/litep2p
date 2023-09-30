// Copyright 2021 Parity Technologies (UK) Ltd.
// Copyright 2022 Protocol Labs.
// Copyright 2023 litep2p developers
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! QUIC transport.

use crate::{
    crypto::tls::{make_client_config, make_server_config},
    error::{AddressError, Error},
    transport::{
        manager::{TransportHandle, TransportManagerCommand},
        quic::config::TransportConfig as QuicTransportConfig,
        Transport,
    },
    types::ConnectionId,
    PeerId,
};

use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use multiaddr::{Multiaddr, Protocol};
use quinn::{ClientConfig, Connecting, Connection, Endpoint, ServerConfig};

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

pub(crate) use substream::Substream;

mod connection;
mod substream;

pub mod config;

/// Logging target for the file.
const LOG_TARGET: &str = "litep2p::quic";

#[derive(Debug)]
struct NegotiatedConnection {
    /// Remote peer ID.
    peer: PeerId,

    /// QUIC connection.
    connection: Connection,
}

/// QUIC transport object.
#[derive(Debug)]
pub(crate) struct QuicTransport {
    /// QUIC server.
    server: Endpoint,

    /// Transport handle.
    context: TransportHandle,

    /// Assigned listen address.
    listen_address: SocketAddr,

    /// Listen address assigned for clients.
    client_listen_address: SocketAddr,

    /// Pending dials.
    pending_dials: HashMap<ConnectionId, Multiaddr>,

    /// Pending connections.
    pending_connections:
        FuturesUnordered<BoxFuture<'static, (ConnectionId, Result<NegotiatedConnection, Error>)>>,
}

impl QuicTransport {
    /// Extract socket address and `PeerId`, if found, from `address`.
    fn get_socket_address(address: &Multiaddr) -> crate::Result<(SocketAddr, Option<PeerId>)> {
        tracing::trace!(target: LOG_TARGET, ?address, "parse multi address");

        let mut iter = address.iter();
        let socket_address = match iter.next() {
            Some(Protocol::Ip6(address)) => match iter.next() {
                Some(Protocol::Udp(port)) => SocketAddr::new(IpAddr::V6(address), port),
                protocol => {
                    tracing::error!(
                        target: LOG_TARGET,
                        ?protocol,
                        "invalid transport protocol, expected `QuicV1`",
                    );
                    return Err(Error::AddressError(AddressError::InvalidProtocol));
                }
            },
            Some(Protocol::Ip4(address)) => match iter.next() {
                Some(Protocol::Udp(port)) => SocketAddr::new(IpAddr::V4(address), port),
                protocol => {
                    tracing::error!(
                        target: LOG_TARGET,
                        ?protocol,
                        "invalid transport protocol, expected `QuicV1`",
                    );
                    return Err(Error::AddressError(AddressError::InvalidProtocol));
                }
            },
            protocol => {
                tracing::error!(target: LOG_TARGET, ?protocol, "invalid transport protocol");
                return Err(Error::AddressError(AddressError::InvalidProtocol));
            }
        };

        // verify that quic exists
        match iter.next() {
            Some(Protocol::QuicV1) => {}
            _ => return Err(Error::AddressError(AddressError::InvalidProtocol)),
        }

        let maybe_peer = match iter.next() {
            Some(Protocol::P2p(multihash)) => Some(PeerId::from_multihash(multihash)?),
            None => None,
            protocol => {
                tracing::error!(
                    target: LOG_TARGET,
                    ?protocol,
                    "invalid protocol, expected `P2p` or `None`"
                );
                return Err(Error::AddressError(AddressError::InvalidProtocol));
            }
        };

        Ok((socket_address, maybe_peer))
    }

    /// Accept QUIC conenction.
    async fn accept_connection(
        &mut self,
        connection_id: ConnectionId,
        connection: Connecting,
    ) -> crate::Result<()> {
        self.pending_connections.push(Box::pin(async move {
            let connection = match connection.await {
                Ok(connection) => connection,
                Err(error) => return (connection_id, Err(error.into())),
            };

            let Some(peer) = Self::extract_peer_id(&connection) else {
                return (connection_id, Err(Error::InvalidCertificate));
            };

            (connection_id, Ok(NegotiatedConnection { peer, connection }))
        }));

        Ok(())
    }

    /// Attempt to extract `PeerId` from connection certificates.
    fn extract_peer_id(connection: &Connection) -> Option<PeerId> {
        let certificates: Box<Vec<rustls::Certificate>> =
            connection.peer_identity()?.downcast().ok()?;
        let p2p_cert = crate::crypto::tls::certificate::parse(certificates.get(0)?)
            .expect("the certificate was validated during TLS handshake; qed");

        Some(p2p_cert.peer_id())
    }

    /// Handle established connection.
    async fn on_connection_established(
        &mut self,
        connection_id: ConnectionId,
        result: crate::Result<NegotiatedConnection>,
    ) -> crate::Result<()> {
        tracing::debug!(target: LOG_TARGET, ?connection_id, success = result.is_ok(), "connection established");

        // `on_connection_established()` is called for both inbound and outbound connections
        // but `pending_dials` will only contain entries for outbound connections.
        let maybe_address = self.pending_dials.remove(&connection_id);

        match result {
            Ok(connection) => {
                let address = maybe_address.map_or(
                    {
                        let address = connection.connection.remote_address();
                        Multiaddr::empty()
                            .with(Protocol::from(address.ip()))
                            .with(Protocol::Udp(address.port()))
                            .with(Protocol::QuicV1)
                    },
                    |address| address,
                );

                let bandwidth_sink = self.context.bandwidth_sink.clone();
                let mut protocol_set = self.context.protocol_set();
                protocol_set
                    .report_connection_established(connection_id, connection.peer, address)
                    .await?;

                tokio::spawn(
                    connection::Connection::new(
                        connection.peer,
                        connection_id,
                        connection.connection,
                        protocol_set,
                        bandwidth_sink,
                    )
                    .start(),
                );
            }
            Err(error) => {
                tracing::debug!(target: LOG_TARGET, ?connection_id, ?error, "failed to establish connection");

                // since the address was found from `pending_dials`,
                // report the error to protocols and `TransportManager`
                if let Some(address) = maybe_address {
                    self.context.report_dial_failure(connection_id, address, error).await;
                }
            }
        }

        Ok(())
    }

    /// Dial remote peer.
    async fn on_dial_peer(
        &mut self,
        address: Multiaddr,
        connection_id: ConnectionId,
    ) -> crate::Result<()> {
        let Ok((socket_address, Some(peer))) = Self::get_socket_address(&address) else {
            return Err(Error::AddressError(AddressError::PeerIdMissing));
        };

        let crypto_config =
            Arc::new(make_client_config(&self.context.keypair, Some(peer)).expect("to succeed"));
        let client_config = ClientConfig::new(crypto_config);
        let client = Endpoint::client(self.client_listen_address)
            .map_err(|error| Error::Other(error.to_string()))?;
        let connection = client
            .connect_with(client_config, socket_address, "l")
            .map_err(|error| Error::Other(error.to_string()))?;

        self.pending_dials.insert(connection_id, address);
        self.pending_connections.push(Box::pin(async move {
            let connection = match connection.await {
                Ok(connection) => connection,
                Err(error) => return (connection_id, Err(error.into())),
            };

            let Some(peer) = Self::extract_peer_id(&connection) else {
                return (connection_id, Err(Error::InvalidCertificate));
            };

            (connection_id, Ok(NegotiatedConnection { peer, connection }))
        }));

        Ok(())
    }
}

#[async_trait::async_trait]
impl Transport for QuicTransport {
    type Config = QuicTransportConfig;

    /// Create new [`QuicTransport`] object.
    async fn new(context: TransportHandle, config: Self::Config) -> crate::Result<Self>
    where
        Self: Sized,
    {
        tracing::info!(
            target: LOG_TARGET,
            listen_address = ?config.listen_address,
            "start quic transport",
        );

        let (listen_address, _) = Self::get_socket_address(&config.listen_address)?;
        let crypto_config = Arc::new(make_server_config(&context.keypair).expect("to succeed"));
        let server_config = ServerConfig::with_crypto(crypto_config);

        let server = Endpoint::server(server_config, listen_address).unwrap();

        let listen_address = server.local_addr()?;
        let client_listen_address = match listen_address.ip() {
            std::net::IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            std::net::IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        };

        Ok(Self {
            server,
            context,
            listen_address,
            client_listen_address,
            pending_dials: HashMap::new(),
            pending_connections: FuturesUnordered::new(),
        })
    }

    /// Get assigned listen address.
    fn listen_address(&self) -> Multiaddr {
        let mut multiaddr = Multiaddr::from(self.listen_address.ip());
        multiaddr.push(Protocol::Udp(self.listen_address.port()));
        multiaddr.push(Protocol::QuicV1);

        multiaddr
    }

    /// Start [`QuicTransport`] event loop.
    async fn start(mut self) -> crate::Result<()> {
        loop {
            tokio::select! {
                connection = self.server.accept() => match connection {
                    Some(connection) => {
                        let connection_id = self.context.next_connection_id();

                        if let Err(error) = self.accept_connection(connection_id, connection).await {
                            tracing::error!(target: LOG_TARGET, ?error, "failed to accept quic connection");
                            return Err(error);
                        }
                    },
                    None => {
                        tracing::error!(target: LOG_TARGET, "failed to accept connection, closing quic transport");
                        return Ok(())
                    }
                },
                connection = self.pending_connections.select_next_some(), if !self.pending_connections.is_empty() => {
                    let (connection_id, result) = connection;

                    if let Err(error) = self.on_connection_established(connection_id, result).await {
                        tracing::debug!(target: LOG_TARGET, ?connection_id, ?error, "failed to handle established connection");
                    }
                }
                command = self.context.next() => match command.ok_or(Error::EssentialTaskClosed)? {
                    TransportManagerCommand::Dial { address, connection } => {
                        if let Err(error) = self.on_dial_peer(address.clone(), connection).await {
                            tracing::debug!(target: LOG_TARGET, ?address, ?connection, ?error, "failed to dial peer");
                            let _ = self.context.report_dial_failure(connection, address, error).await;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codec::ProtocolCodec,
        crypto::{ed25519::Keypair, PublicKey},
        transport::manager::{ProtocolContext, TransportManagerEvent},
        types::protocol::ProtocolName,
        BandwidthSink,
    };
    use multihash::Multihash;
    use tokio::sync::mpsc::channel;

    #[tokio::test]
    async fn test_quinn() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();

        let keypair1 = Keypair::generate();
        let (tx1, _rx1) = channel(64);
        let (event_tx1, mut event_rx1) = channel(64);
        let (_cmd_tx1, cmd_rx1) = channel(64);

        let handle1 = crate::transport::manager::TransportHandle {
            protocol_names: Vec::new(),
            next_substream_id: Default::default(),
            next_connection_id: Default::default(),
            keypair: keypair1.clone(),
            tx: event_tx1,
            rx: cmd_rx1,
            bandwidth_sink: BandwidthSink::new(),

            protocols: HashMap::from_iter([(
                ProtocolName::from("/notif/1"),
                ProtocolContext {
                    tx: tx1,
                    codec: ProtocolCodec::Identity(32),
                    fallback_names: Vec::new(),
                },
            )]),
        };
        let transport_config1 = QuicTransportConfig {
            listen_address: "/ip6/::1/udp/0/quic-v1".parse().unwrap(),
        };

        let transport1 = QuicTransport::new(handle1, transport_config1).await.unwrap();

        let listen_address = Transport::listen_address(&transport1);

        tokio::spawn(async move {
            let _ = transport1.start().await;
        });

        let keypair2 = Keypair::generate();
        let (tx2, _rx2) = channel(64);
        let (event_tx2, mut event_rx2) = channel(64);
        let (cmd_tx2, cmd_rx2) = channel(64);

        let handle2 = crate::transport::manager::TransportHandle {
            protocol_names: Vec::new(),
            next_substream_id: Default::default(),
            next_connection_id: Default::default(),
            keypair: keypair2.clone(),
            tx: event_tx2,
            rx: cmd_rx2,
            bandwidth_sink: BandwidthSink::new(),

            protocols: HashMap::from_iter([(
                ProtocolName::from("/notif/1"),
                ProtocolContext {
                    tx: tx2,
                    codec: ProtocolCodec::Identity(32),
                    fallback_names: Vec::new(),
                },
            )]),
        };
        let transport_config2 = QuicTransportConfig {
            listen_address: "/ip6/::1/udp/0/quic-v1".parse().unwrap(),
        };

        let transport2 = QuicTransport::new(handle2, transport_config2).await.unwrap();

        tokio::spawn(async move {
            let _ = transport2.start().await;
        });

        let peer1: PeerId = PeerId::from_public_key(&PublicKey::Ed25519(keypair1.public()));
        let _peer2: PeerId = PeerId::from_public_key(&PublicKey::Ed25519(keypair2.public()));
        let listen_address = listen_address.with(Protocol::P2p(
            Multihash::from_bytes(&peer1.to_bytes()).unwrap(),
        ));

        cmd_tx2
            .send(TransportManagerCommand::Dial {
                address: listen_address,
                connection: ConnectionId::new(),
            })
            .await
            .unwrap();

        let (res1, res2) = tokio::join!(event_rx1.recv(), event_rx2.recv());

        assert!(std::matches!(
            res1,
            Some(TransportManagerEvent::ConnectionEstablished { .. })
        ));
        assert!(std::matches!(
            res2,
            Some(TransportManagerEvent::ConnectionEstablished { .. })
        ));
    }
}
