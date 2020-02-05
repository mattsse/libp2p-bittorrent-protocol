use crate::piece::PieceCheckerState;
use crate::torrent::MetaInfo;
pub struct MetainfoState {
    file: MetaInfo,
    state: PieceCheckerState,
}
