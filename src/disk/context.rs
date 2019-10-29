use crate::disk::fs::FileSystem;
use crate::disk::message::DiskMessageOut;
use crate::piece::PieceCheckerState;
use crate::torrent::MetaInfo;
use crate::util::ShaHash;
use futures::sync::mpsc::Sender;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

pub struct DiskManagerContext<TFileSystem: FileSystem> {
    torrents: Arc<RwLock<HashMap<ShaHash, Mutex<MetainfoState>>>>,
    out: Sender<DiskMessageOut>,
    file_system: Arc<TFileSystem>,
}

pub struct MetainfoState {
    file: MetaInfo,
    state: PieceCheckerState,
}
