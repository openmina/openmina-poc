#![forbid(unsafe_code)]

mod state;

use std::io;

use libp2p::{futures::StreamExt, swarm::ConnectionId};
use mina_p2p_messages::rpc_kernel::{RpcMethod, ResponsePayload, MessageHeader, Error, NeedsLength};

pub struct Engine {
    swarm: libp2p::Swarm<mina_transport::Behaviour>,
    state: state::P2pState,
}

impl Engine {
    pub fn new(swarm: libp2p::Swarm<mina_transport::Behaviour>) -> Self {
        Engine {
            swarm,
            state: state::P2pState::default(),
        }
    }

    async fn drive(&mut self) -> Option<state::Event> {
        if let Some(event) = self.swarm.next().await {
            self.state.on_event(event)
        } else {
            None
        }
    }

    pub async fn rpc<M: RpcMethod>(
        &mut self,
        query: M::Query,
    ) -> Result<Result<M::Response, Error>, binprot::Error> {
        let (peer_id, mut ctx) = if let Some(peer_id) = self.state.cns().keys().next().cloned() {
            (
                peer_id,
                self.state.cns().remove(&peer_id).expect("checked above"),
            )
        } else {
            loop {
                match self.drive().await {
                    Some(state::Event::ReadyToWrite(peer_id, ctx)) => break (peer_id, ctx),
                    _ => {}
                }
            }
        };

        let bytes = ctx.make::<M>(query);
        let connection_id = ConnectionId::new_unchecked(ctx.id());
        self.state.cns().insert(peer_id, ctx);
        self.swarm
            .behaviour_mut()
            .rpc
            .send(peer_id, connection_id, bytes);

        'drive: loop {
            match self.drive().await {
                Some(state::Event::ReadyToRead(this_peer_id, mut ctx)) => {
                    if peer_id == this_peer_id {
                        loop {
                            match ctx.read_header() {
                                Err(binprot::Error::IoError(err))
                                    if err.kind() == io::ErrorKind::WouldBlock =>
                                {
                                    self.state.cns().insert(peer_id, ctx);
                                    continue 'drive;
                                }
                                Err(err) => return Err(err),
                                Ok(MessageHeader::Heartbeat) => {
                                    self.swarm.behaviour_mut().rpc.send(
                                        peer_id,
                                        connection_id,
                                        b"\x01\x00\x00\x00\x00\x00\x00\x00\x00".to_vec(),
                                    );
                                }
                                Ok(MessageHeader::Query(q)) => {
                                    // TODO: process query
                                    use mina_p2p_messages::rpc::VersionedRpcMenuV1;

                                    let tag = std::str::from_utf8(q.tag.as_ref()).unwrap();
                                    match (tag, q.version) {
                                        (VersionedRpcMenuV1::NAME, VersionedRpcMenuV1::VERSION) => {
                                            let bytes = ctx
                                                .make_response::<VersionedRpcMenuV1>(vec![], q.id);
                                            self.swarm.behaviour_mut().rpc.send(
                                                peer_id,
                                                connection_id,
                                                bytes,
                                            );
                                        }
                                        _ => unimplemented!(),
                                    }
                                }
                                Ok(MessageHeader::Response(h)) => {
                                    if h.id == i64::from_le_bytes(*b"RPC\x00\x00\x00\x00\x00") {
                                        ctx.read_remaining::<u8>()?;
                                        // TODO: process this message
                                    } else {
                                        let r = ctx.read_remaining::<ResponsePayload<_>>()?;
                                        self.state.cns().insert(peer_id, ctx);
                                        return Ok(r.0.map(|NeedsLength(x)| x));
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}