/// Status of our connection to a node reported by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BtConnectionType {
    /// Sender hasn't tried to connect to peer.
    NotConnected = 0,
    /// Sender is currently connected to peer.
    Connected = 1,
}

/// Status of our interest to download a target by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BtInterestType {
    /// Sender has no interest to download.
    NotInterested = 0,
    /// Sender is currently interested to download.
    Interested = 1,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum TrackerResponseKeyword {
    Must,
    MustNot,
    Required,
    Shall,
    ShallNot,
    Should,
    ShouldNot,
    Recommended,
    May,
    Optional,
}

/// Info about a peer returned by a tracker
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct PeerInfo {
    /// identification of the peer
    id: [u8; 20],
    /// domain name or an IP address of the peer
    ip: String,
    /// port number of the peer
    port: u8,
}

/// Return format of the peer info returned BitTorrent tracker.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BtPeerInfoFormat {
    NotCompact = 0,
    Compact = 1,
}
