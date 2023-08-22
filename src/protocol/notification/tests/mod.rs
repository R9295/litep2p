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

use crate::{
    crypto::ed25519::Keypair,
    peer_id::PeerId,
    protocol::{
        notification::{handle::NotificationHandle, types::Config, NotificationProtocol},
        ProtocolCommand, TransportService,
    },
    transport::manager::TransportManager,
    types::protocol::ProtocolName,
};

use tokio::sync::mpsc::{channel, Receiver};

#[cfg(test)]
mod notification;
#[cfg(test)]
mod substream_validation;

/// create new `NotificationProtocol`
fn make_notification_protocol() -> (NotificationProtocol, NotificationHandle, TransportManager) {
    let (manager, handle) = TransportManager::new(Keypair::generate());

    let peer = PeerId::random();
    let (transport_service, _tx) = TransportService::new(
        peer,
        ProtocolName::from("/kad/1"),
        std::sync::Arc::new(Default::default()),
        handle,
    );
    let (config, handle) = Config::new(
        ProtocolName::from("/notif/1"),
        1024usize,
        vec![1, 2, 3, 4],
        Vec::new(),
    );

    (
        NotificationProtocol::new(transport_service, config),
        handle,
        manager,
    )
}

/// add new peer to `NotificationProtocol`
fn add_peer() -> (PeerId, (), Receiver<ProtocolCommand>) {
    let (_tx, rx) = channel(64);

    (PeerId::random(), (), rx)
}
