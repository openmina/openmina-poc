use std::{
    fs::{File, self},
    path::Path,
    collections::{BTreeMap, BTreeSet},
};

use libp2p::{futures::StreamExt, swarm::SwarmEvent};
use mina_p2p_messages::{
    rpc::{
        GetBestTipV2, GetAncestryV2, GetStagedLedgerAuxAndPendingCoinbasesAtHashV2,
        AnswerSyncLedgerQueryV2, GetTransitionChainV2, GetTransitionChainProofV1ForV2,
    },
    rpc_kernel::{RpcMethod, QueryHeader, QueryPayload, RpcResult},
    v2,
};
use binprot::BinProtRead;
use libp2p_rpc_behaviour::{Event, Received, Behaviour};

use super::snarked_ledger::SnarkedLedger;

pub async fn run(mut swarm: libp2p::Swarm<Behaviour>, path_main: &Path, height: u32) {
    let path_blocks = path_main.join("blocks");
    let path = path_main.join(height.to_string());

    let mut file = File::open(path.join("best_tip")).unwrap();
    let best_tip = <GetBestTipV2 as RpcMethod>::Response::binprot_read(&mut file).unwrap();

    let mut file = File::open(path.join("ancestry")).unwrap();
    let ancestry = <GetAncestryV2 as RpcMethod>::Response::binprot_read(&mut file).unwrap();

    let mut file = File::open(path.join("staged_ledger_aux")).unwrap();
    type T = GetStagedLedgerAuxAndPendingCoinbasesAtHashV2;
    let staged_ledger_aux = <T as RpcMethod>::Response::binprot_read(&mut file).unwrap();

    let mut ledgers = BTreeMap::new();
    for entry in fs::read_dir(path.join("ledgers")).unwrap() {
        let entry = entry.unwrap();
        let file = File::open(entry.path()).unwrap();
        let ledger = SnarkedLedger::load_bin(file).unwrap();
        ledgers.insert(entry.file_name().to_str().unwrap().to_string(), ledger);
    }

    let file = File::open(path_main.join("blocks").join("table.json")).unwrap();
    let table = serde_json::from_reader::<_, BTreeMap<String, u32>>(file).unwrap();

    let mut peers = BTreeSet::default();

    while let Some(event) = swarm.next().await {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                log::info!("listen on {address}");
            }
            SwarmEvent::Behaviour((peer_id, Event::ConnectionEstablished)) => {
                peers.insert(peer_id);
                log::info!("new connection {peer_id}");
            }
            SwarmEvent::Behaviour((peer_id, Event::ConnectionClosed)) => {
                log::info!("connection closed {peer_id}");
                peers.remove(&peer_id);
            }
            SwarmEvent::Behaviour((
                peer_id,
                Event::Stream {
                    stream_id,
                    received,
                },
            )) => match received {
                Received::HandshakeDone => {
                    log::info!("new stream {peer_id} {stream_id:?}");
                }
                Received::Menu(menu) => {
                    log::info!("menu: {menu:?}");
                }
                Received::Query {
                    header: QueryHeader { tag, version, id },
                    bytes,
                } => {
                    let mut bytes = bytes.as_slice();
                    let tag = std::str::from_utf8(tag.as_ref()).unwrap();
                    log::info!("handling {tag}, {}", version);
                    match (tag, version) {
                        (GetBestTipV2::NAME, GetBestTipV2::VERSION) => {
                            swarm
                                .behaviour_mut()
                                .respond::<GetBestTipV2>(
                                    peer_id,
                                    stream_id,
                                    id,
                                    Ok(best_tip.clone()),
                                )
                                .unwrap();
                        }
                        (GetAncestryV2::NAME, GetAncestryV2::VERSION) => {
                            swarm
                                .behaviour_mut()
                                .respond::<GetAncestryV2>(
                                    peer_id,
                                    stream_id,
                                    id,
                                    Ok(ancestry.clone()),
                                )
                                .unwrap();
                        }
                        (AnswerSyncLedgerQueryV2::NAME, AnswerSyncLedgerQueryV2::VERSION) => {
                            type T = AnswerSyncLedgerQueryV2;
                            let (hash, query) =
                                QueryPayload::<<T as RpcMethod>::Query>::binprot_read(&mut bytes)
                                    .unwrap()
                                    .0;

                            let hash = v2::LedgerHash::from(v2::MinaBaseLedgerHash0StableV1(hash));
                            let hash_str = match serde_json::to_value(&hash).unwrap() {
                                serde_json::Value::String(s) => s,
                                _ => panic!(),
                            };

                            let ledger = ledgers.get_mut(&hash_str).unwrap();
                            let response = ledger.serve_query(query);

                            swarm
                                .behaviour_mut()
                                .respond::<T>(peer_id, stream_id, id, Ok(RpcResult(Ok(response))))
                                .unwrap();
                        }
                        (
                            GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME,
                            GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::VERSION,
                        ) => swarm
                            .behaviour_mut()
                            .respond::<GetStagedLedgerAuxAndPendingCoinbasesAtHashV2>(
                                peer_id,
                                stream_id,
                                id,
                                Ok(staged_ledger_aux.clone()),
                            )
                            .unwrap(),
                        (GetTransitionChainV2::NAME, GetTransitionChainV2::VERSION) => {
                            type T = GetTransitionChainV2;
                            let hashes =
                                QueryPayload::<<T as RpcMethod>::Query>::binprot_read(&mut bytes)
                                    .unwrap()
                                    .0;

                            // let mut contains_last = false;
                            let response = hashes
                                .into_iter()
                                .map(|hash| {
                                    let hash =
                                        v2::StateHash::from(v2::DataHashLibStateHashStableV1(hash));
                                    // if hash
                                    //     == best_tip
                                    //         .as_ref()
                                    //         .unwrap()
                                    //         .data
                                    //         .header
                                    //         .protocol_state
                                    //         .previous_state_hash
                                    // {
                                    //     contains_last = true;
                                    // }
                                    let height = table.get(&hash.to_string()).unwrap();
                                    let path =
                                        path_blocks.join(height.to_string()).join(hash.to_string());
                                    let mut file = File::open(path).unwrap();
                                    binprot::BinProtRead::binprot_read(&mut file).unwrap()
                                })
                                .collect();
                            swarm
                                .behaviour_mut()
                                .respond::<T>(peer_id, stream_id, id, Ok(Some(response)))
                                .unwrap();
                            // if contains_last {
                            //     swarm.disconnect_peer_id(peer_id).unwrap();
                            // }
                        }
                        (
                            GetTransitionChainProofV1ForV2::NAME,
                            GetTransitionChainProofV1ForV2::VERSION,
                        ) => {
                            type T = GetTransitionChainProofV1ForV2;
                            let hash =
                                QueryPayload::<<T as RpcMethod>::Query>::binprot_read(&mut bytes)
                                    .unwrap()
                                    .0;

                            let hash = v2::StateHash::from(v2::DataHashLibStateHashStableV1(hash));
                            let response = if let Some(height) = table.get(&hash.to_string()) {
                                let path = path_blocks
                                    .join(height.to_string())
                                    .join(format!("proof_{hash}"));
                                let mut file = File::open(path).unwrap();
                                binprot::BinProtRead::binprot_read(&mut file).unwrap()
                            } else {
                                log::warn!("no proof for block {hash}");
                                None
                            };

                            swarm
                                .behaviour_mut()
                                .respond::<T>(peer_id, stream_id, id, Ok(response))
                                .unwrap();
                        }
                        (name, version) => {
                            log::warn!("TODO: unhandled {name}, {version}");
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}
