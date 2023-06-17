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

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

/// Protocol name.
#[derive(Debug, Clone)]
pub enum ProtocolName {
    Static(&'static str),
    Allocated(Arc<String>),
}

impl ProtocolName {
    pub const fn from_static_str(name: &'static str) -> Self {
        ProtocolName::Static(name)
    }
}

impl From<&'static str> for ProtocolName {
    fn from(protocol: &'static str) -> Self {
        ProtocolName::Static(protocol)
    }
}

impl From<String> for ProtocolName {
    fn from(protocol: String) -> Self {
        ProtocolName::Allocated(Arc::new(protocol))
    }
}

impl std::ops::Deref for ProtocolName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Static(protocol) => protocol,
            Self::Allocated(protocol) => protocol.as_str(),
        }
    }
}

impl Hash for ProtocolName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self as &str).hash(state)
    }
}

impl PartialEq for ProtocolName {
    fn eq(&self, other: &Self) -> bool {
        (self as &str) == (other as &str)
    }
}

impl Eq for ProtocolName {}
