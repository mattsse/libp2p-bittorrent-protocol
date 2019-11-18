use crate::behavior::SeedLeechConfig;
use crate::bitfield::BitField;
use crate::peer::torrent::Torrent;
use crate::peer::TorrentState;
use crate::util::ShaHash;
use crate::MetaInfo;

pub struct TorrentConfig {
    pub info_hash: ShaHash,
    pub state: TorrentState,
    pub bitfield: BitField,
    pub piece_length: u64,
    pub last_piece_length: u64,
    pub seed_leech: SeedLeechConfig,
}

impl TorrentConfig {
    pub fn new_leech(torrent: &MetaInfo, state: TorrentState) -> Self {
        TorrentBuilder::default()
            .info_hash(torrent.info_hash.clone())
            .bitfield(BitField::new_all_clear(torrent.info.pieces.len()))
            .state(state)
            .piece_length(torrent.info.piece_length)
            .last_piece_length(torrent.info.last_piece_length())
            .into_leech()
            .unwrap()
    }

    pub fn new_seed(torrent: &MetaInfo, state: TorrentState) -> Self {
        TorrentBuilder::default()
            .info_hash(torrent.info_hash.clone())
            .bitfield(BitField::new_all_set(torrent.info.pieces.len()))
            .state(state)
            .piece_length(torrent.info.piece_length)
            .last_piece_length(torrent.info.last_piece_length())
            .into_seed()
            .unwrap()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TorrentBuilder {
    state: Option<TorrentState>,
    bitfield: Option<BitField>,
    info_hash: Option<ShaHash>,
    piece_length: Option<u64>,
    last_piece_length: Option<u64>,
}

impl TorrentBuilder {
    pub fn state(mut self, state: TorrentState) -> Self {
        self.state = Some(state);
        self
    }

    pub fn bitfield(mut self, bitfield: BitField) -> Self {
        self.bitfield = Some(bitfield);
        self
    }

    pub fn info_hash<T: Into<ShaHash>>(mut self, info_hash: T) -> Self {
        self.info_hash = Some(info_hash.into());
        self
    }

    pub fn piece_length(mut self, piece_length: u64) -> Self {
        self.piece_length = Some(piece_length);
        self
    }

    pub fn last_piece_length(mut self, last_piece_length: u64) -> Self {
        self.last_piece_length = Some(last_piece_length);
        self
    }

    pub fn into_seed(self) -> Result<TorrentConfig, String> {
        self.build(SeedLeechConfig::SeedOnly)
    }

    pub fn build(self, seed_leech: SeedLeechConfig) -> Result<TorrentConfig, String> {
        let state = self.state.unwrap_or_default();
        let bitfield = self
            .bitfield
            .ok_or_else(|| "field bitfield must be? initialized.".to_string())?;
        let info_hash = self
            .info_hash
            .ok_or_else(|| "field info_hash must be initialized.".to_string())?;
        let piece_length = self
            .piece_length
            .ok_or_else(|| "field piece_length? must be initialized.".to_string())?;
        let last_piece_length = self
            .last_piece_length
            .ok_or_else(|| "field? last_piece_length must be initialized.".to_string())?;

        Ok(TorrentConfig {
            state,
            bitfield,
            info_hash,
            piece_length,
            last_piece_length,
            seed_leech,
        })
    }

    pub fn into_leech(self) -> Result<TorrentConfig, String> {
        self.build(SeedLeechConfig::SeedAndLeech)
    }
}
