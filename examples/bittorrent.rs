//! A basic example demonstrating torrenting over libp2p.
//!
//! In the first terminal window, run:
//!
//! ```sh
//! cargo run --example bittorrent
//! ```
//!
//! It will print the PeerId and the listening address, e.g. `Listening on
//! Local peer id: PeerId("QmQodo6YFTB2CGTSgFHH1JLmFPmD4EwTgeXsqLGsnBGgYb")
//! "/ip4/0.0.0.0/tcp/24345"`
//!
//! It will then generate some random file to seed and prints the path to the
//! torrent file "/var/folders/l5/lprhf80000gn/T/.tmpQNd/rubbish.torrent"
//!
//! In the second terminal window, start a new instance of the example with:
//!
//! ```sh
//! cargo run --example bittorrent -- /ip4/127.0.0.1/tcp/24345 "QmQodo6YFTB2CGTSgFHH1JLmFPmD4EwTgeXsqLGsnBGgYb" "/var/folders/l5/lprhf80000gn/T/.tmpQNd/rubbish.torrent"
//! ```
//!
//! The two nodes establish a connection, negotiate the bittorent protocol
//! and begin torrenting the demo file.

use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::{
    error::Error,
    task::{Context, Poll},
};

use futures::prelude::*;
use libp2p::{identity, PeerId, Swarm};
use rand;
use tempfile::tempdir;

use libp2p_bittorrent::behavior::InterestOk;
use libp2p_bittorrent::disk::NativeFileSystem;
use libp2p_bittorrent::peer::TorrentState;
use libp2p_bittorrent::{
    BitTorrent,
    BitTorrentConfig,
    BitTorrentEvent,
    MetaInfo,
    TorrentBuilder,
    TorrentSeed,
};

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    // Create a random key for ourselves.
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("Local peer id: {:?}", local_peer_id);
    let peer_id = local_peer_id.to_string();

    // Set up a an encrypted DNS-enabled TCP Transport over the Mplex protocol
    let transport = libp2p::build_development_transport(local_key)?;

    let tmp_dir = tempdir().expect("Failed to create temp dir");
    println!("Initialized tmp dir {}", tmp_dir.path().display());

    let native_fs = NativeFileSystem::from(tmp_dir.path());
    let config = BitTorrentConfig::default();
    let behaviour = BitTorrent::with_config(local_peer_id.clone(), native_fs, config);

    let mut swarm = Swarm::new(transport, behaviour, local_peer_id);

    let mut torrent_file = "".to_string();

    // Order BitTorrent to start torrenting from a peer.
    if let Some(addr) = env::args().nth(1) {
        let peer_id = env::args().nth(2).expect("Demo torrent required.");
        let peer_id: PeerId = peer_id.parse().expect("Failed to parse peer ID to find");
        let torrent = env::args().nth(3).expect("Demo torrent required.");
        let torrent = MetaInfo::from_torrent_file(torrent).expect("Failed to load torrent file");
        let info_hash = torrent.info_hash.clone();

        // add a new leech
        swarm.add_leech(torrent, TorrentState::Active);

        // start the handshake
        swarm.add_address_torrents(
            peer_id.clone(),
            addr.parse().expect("Failed to parse multiaddr."),
            &[info_hash.clone().into()],
        );

        swarm
            .handshake_known_peer(peer_id, info_hash)
            .expect("Peer id needs to be known.");
    } else {
        // generate some rubbish

        let seed = tmp_dir.path().join("rubbish.txt");

        let mut seed_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&seed)
            .expect("Failed to create temp file");

        // write 160KB to the file
        for _ in 0..10 {
            let mut buf: Vec<u8> = (0..16384).map(|_| rand::random::<u8>()).collect();
            seed_file
                .write_all(&mut buf)
                .expect("Failed to write to file");
        }

        let meta_info = TorrentBuilder::new(&seed)
            .build()
            .expect("Failed to generate torrent.");

        let torrent = tmp_dir.path().join("rubbish.torrent");
        meta_info
            .write_torrent_file(&torrent)
            .expect("Failed to save torrent file");

        println!("Torrentfile: {}", torrent.display());
        torrent_file = format!("{}", torrent.display());

        let seed = TorrentSeed::new(seed, meta_info);

        // add the seed
        swarm.add_seed(seed, TorrentState::Active);
    };

    // Tell the swarm to listen on all interfaces and a random, OS-assigned port.
    Swarm::listen_on(&mut swarm, "/ip4/0.0.0.0/tcp/0".parse().unwrap()).unwrap();

    // Use tokio to drive the `Swarm`.
    let mut listening = false;

    // Start torrenting
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(futures::future::poll_fn(move |cx: &mut Context| loop {
            match swarm.poll_next_unpin(cx) {
                Poll::Ready(Some(BitTorrentEvent::TorrentAddedResult(res))) => match res {
                    Ok(ok) => {
                        println!("Added new Seed: {:?}", ok);
                    }
                    Err(err) => {
                        println!("Failed to add new seed: {:?}", err);
                    }
                },
                Poll::Ready(Some(BitTorrentEvent::HandshakeResult(res))) => match res {
                    Ok(ok) => {
                        println!("Handshake ok: {:?}", ok);
                    }
                    Err(err) => {
                        println!("Failed to handshake: {:?}", err);
                    }
                },
                Poll::Ready(Some(BitTorrentEvent::InterestResult(res))) => match res {
                    Ok(InterestOk::Interested(peer)) => {
                        swarm.unchoke_peer(&peer);
                        println!("interested: {:?}", peer);
                    }
                    Ok(InterestOk::NotInterested(peer)) => {
                        println!("not interested: {:?}", peer);
                    }
                    Err(err) => {
                        println!("interest error: {:?}", err);
                    }
                },
                Poll::Ready(Some(BitTorrentEvent::BitfieldResult(res))) => match res {
                    Ok(ok) => {
                        println!("bitfield ok: {:?}", ok);
                    }
                    Err(err) => {
                        println!("bitfield error: {:?}", err);
                    }
                },
                Poll::Ready(Some(BitTorrentEvent::ChokeResult(res))) => match res {
                    Ok(ok) => {
                        println!("choke ok: {:?}", ok);
                    }
                    Err(err) => {
                        println!("choke error: {:?}", err);
                    }
                },
                Poll::Ready(Some(BitTorrentEvent::HaveResult(res))) => match res {
                    Ok(ok) => {
                        println!("have ok: {:?}", ok);
                    }
                    Err(err) => {
                        println!("have error: {:?}", err);
                    }
                },
                Poll::Ready(Some(BitTorrentEvent::TorrentFinished(torrent))) => {
                    println!("Torrent finished {:?}", torrent);
                }
                Poll::Ready(Some(BitTorrentEvent::DiskResult(disk))) => {
                    println!("Disk result {:?}", disk);
                }
                Poll::Ready(Some(_)) => {}
                Poll::Ready(None) | Poll::Pending => {
                    if !listening {
                        if let Some(a) = Swarm::listeners(&swarm).next() {
                            println!("Listening on {:?}", a);
                            println!(
                                " cargo run --example bittorrent {:?} \"{}\" \"{}\"",
                                a, peer_id, torrent_file
                            );
                            listening = true;
                        }
                    }
                    return Poll::<()>::Pending;
                }
            }
        }));

    Ok(())
}
