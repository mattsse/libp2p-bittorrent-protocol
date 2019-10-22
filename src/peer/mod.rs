/// Status of our connection to a node reported by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum ConnectionType {
    /// Sender hasn't tried to connect to peer.
    NotConnected = 0,
    /// Sender is currently connected to peer.
    Connected = 1,
}

/// Status of our interest to download a target by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum InterestType {
    /// Sender has no interest to download.
    NotInterested = 0,
    /// Sender is currently interested to download.
    Interested = 1,
}

/// Return format of the peer info returned BitTorrent tracker.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum PeerInfoFormat {
    NotCompact = 0,
    Compact = 1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// Identifier of the peer.
    pub peer_id: [u8; 20],
    /// peer's IP address
    pub ip: PeerIp,
    /// peer's port number
    pub port: u32,
}

/// peer's IP address either IPv6 (hexed) or IPv4 (dotted quad) or DNS name (string)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIp {
    Ip4,
    Ip6,
    DNS(String),
}
