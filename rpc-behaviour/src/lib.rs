mod state;

use std::{
    collections::{VecDeque, BTreeMap, BTreeSet},
    task::{Waker, Context, Poll},
    io,
    time::Duration,
    sync::Arc,
};

use libp2p::{
    swarm::{
        ToSwarm, NetworkBehaviour, NotifyHandler, ConnectionId, ConnectionHandler,
        SubstreamProtocol, KeepAlive, FromSwarm, THandlerOutEvent, PollParameters, THandlerInEvent,
        derive_prelude::ConnectionEstablished, ConnectionClosed, ConnectionHandlerEvent,
        handler::ConnectionEvent, THandler, ConnectionDenied,
    },
    PeerId,
    core::{upgrade::ReadyUpgrade, Endpoint, Negotiated, muxing::SubstreamBox},
    Multiaddr,
};

use mina_p2p_messages::{
    rpc_kernel::{
        Message, RpcResult, Query, Response, NeedsLength, Error, RpcMethod, MessageHeader,
        ResponseHeader, ResponsePayload,
    },
    rpc::VersionedRpcMenuV1,
};
use binprot::{BinProtWrite, BinProtRead};

#[derive(Default)]
pub struct BehaviourBuilder {
    menu: BTreeSet<(&'static str, i32)>,
}

impl BehaviourBuilder {
    pub fn register_method<M>(mut self) -> Self
    where
        M: RpcMethod,
    {
        self.menu.insert((M::NAME, M::VERSION));
        self
    }

    pub fn build(self) -> Behaviour {
        Behaviour {
            menu: Arc::new(self.menu),
            ..Default::default()
        }
    }
}

#[derive(Default)]
pub struct Behaviour {
    menu: Arc<BTreeSet<(&'static str, i32)>>,
    peers: BTreeMap<PeerId, ConnectionId>,
    queue: VecDeque<ToSwarm<(PeerId, Event), Command>>,
    pending: BTreeMap<PeerId, VecDeque<Command>>,
    waker: Option<Waker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StreamId {
    Incoming(u32),
    Outgoing(u32),
}

#[derive(Debug)]
pub enum Command {
    Send { stream_id: StreamId, bytes: Vec<u8> },
    Open { outgoing_stream_id: u32 },
}

#[derive(Debug)]
pub enum Event {
    ConnectionEstablished,
    ConnectionClosed,
    StreamNegotiated {
        stream_id: StreamId,
        menu: Vec<(String, i32)>,
    },
    Stream {
        stream_id: StreamId,
        header: MessageHeader,
        bytes: Vec<u8>,
    },
}

impl Behaviour {
    fn dispatch_command(&mut self, peer_id: PeerId, command: Command) {
        if let Some(connection_id) = self.peers.get(&peer_id) {
            self.queue.push_back(ToSwarm::NotifyHandler {
                peer_id,
                handler: NotifyHandler::One(*connection_id),
                event: command,
            });
            self.waker.as_ref().map(Waker::wake_by_ref);
        } else {
            self.pending.entry(peer_id).or_default().push_back(command);
        }
    }

    pub fn open(&mut self, peer_id: PeerId, outgoing_stream_id: u32) {
        self.dispatch_command(peer_id, Command::Open { outgoing_stream_id })
    }

    pub fn respond<M>(
        &mut self,
        peer_id: PeerId,
        stream_id: StreamId,
        id: i64,
        response: Result<M::Response, Error>,
    ) where
        M: RpcMethod,
    {
        let data = RpcResult(response.map(NeedsLength));
        let msg = Message::<M::Response>::Response(Response { id, data });
        let mut bytes = vec![0; 8];
        msg.binprot_write(&mut bytes).unwrap();
        let len = (bytes.len() - 8) as u64;
        bytes[..8].clone_from_slice(&len.to_le_bytes());

        self.dispatch_command(peer_id, Command::Send { stream_id, bytes })
    }

    pub fn query<M>(&mut self, peer_id: PeerId, stream_id: StreamId, id: i64, query: M::Query)
    where
        M: RpcMethod,
    {
        let msg = Message::<M::Query>::Query(Query {
            tag: M::NAME.into(),
            version: M::VERSION,
            id,
            data: NeedsLength(query),
        });
        let mut bytes = vec![0; 8];
        msg.binprot_write(&mut bytes).unwrap();
        let len = (bytes.len() - 8) as u64;
        bytes[..8].clone_from_slice(&len.to_le_bytes());

        self.dispatch_command(peer_id, Command::Send { stream_id, bytes })
    }
}

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler = Handler;
    type OutEvent = (PeerId, Event);

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.peers.insert(peer, connection_id);
        Ok(Handler {
            menu: self.menu.clone(),
            ..Default::default()
        })
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        _addr: &Multiaddr,
        _role_override: Endpoint,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.peers.insert(peer, connection_id);
        Ok(Handler {
            menu: self.menu.clone(),
            ..Default::default()
        })
    }

    fn on_swarm_event(&mut self, event: FromSwarm<Self::ConnectionHandler>) {
        match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                connection_id,
                ..
            }) => {
                self.peers.insert(peer_id, connection_id);
                self.queue.push_back(ToSwarm::GenerateEvent((
                    peer_id,
                    Event::ConnectionEstablished,
                )));
                if let Some(queue) = self.pending.remove(&peer_id) {
                    for command in queue {
                        self.queue.push_back(ToSwarm::NotifyHandler {
                            peer_id,
                            handler: NotifyHandler::One(connection_id),
                            event: command,
                        });
                    }
                }
                self.waker.as_ref().map(Waker::wake_by_ref);
            }
            FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id,
                connection_id,
                ..
            }) => {
                if self.peers.get(&peer_id) == Some(&connection_id) {
                    self.peers.remove(&peer_id);
                }
                self.queue
                    .push_back(ToSwarm::GenerateEvent((peer_id, Event::ConnectionClosed)));
                self.waker.as_ref().map(Waker::wake_by_ref);
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.peers.insert(peer_id, connection_id);
        self.queue
            .push_back(ToSwarm::GenerateEvent((peer_id, event)));
        self.waker.as_ref().map(Waker::wake_by_ref);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        _params: &mut impl PollParameters,
    ) -> Poll<ToSwarm<Self::OutEvent, THandlerInEvent<Self>>> {
        if let Some(event) = self.queue.pop_front() {
            Poll::Ready(event)
        } else {
            self.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[derive(Default)]
pub struct Handler {
    menu: Arc<BTreeSet<(&'static str, i32)>>,
    streams: BTreeMap<StreamId, Stream>,
    last_outgoing_id: VecDeque<u32>,
    last_incoming_id: u32,

    waker: Option<Waker>,
}

struct Stream {
    opening_state: Option<OpeningState>,
    inner_state: state::Inner,
}

enum OpeningState {
    Requested,
    Negotiated { io: Negotiated<SubstreamBox> },
}

impl Handler {
    const PROTOCOL_NAME: [u8; 15] = *b"coda/rpcs/0.0.1";

    fn add_stream(&mut self, incoming: bool, io: Negotiated<SubstreamBox>) {
        let opening_state = Some(OpeningState::Negotiated { io });
        if incoming {
            let id = self.last_incoming_id;
            self.last_incoming_id += 1;
            self.streams.insert(
                StreamId::Incoming(id),
                Stream {
                    opening_state,
                    inner_state: state::Inner::new(self.menu.clone()),
                },
            );
            self.waker.as_ref().map(Waker::wake_by_ref);
        } else if let Some(id) = self.last_outgoing_id.pop_front() {
            if let Some(stream) = self.streams.get_mut(&StreamId::Outgoing(id)) {
                stream.opening_state = opening_state;
                self.waker.as_ref().map(Waker::wake_by_ref);
            }
        }
    }
}

impl ConnectionHandler for Handler {
    type InEvent = Command;
    type OutEvent = Event;
    type Error = io::Error;
    type InboundProtocol = ReadyUpgrade<[u8; 15]>;
    type OutboundProtocol = ReadyUpgrade<[u8; 15]>;
    type OutboundOpenInfo = ();
    type InboundOpenInfo = ();

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(ReadyUpgrade::new(Self::PROTOCOL_NAME), ())
            .with_timeout(Duration::from_secs(15))
    }

    fn connection_keep_alive(&self) -> KeepAlive {
        KeepAlive::Yes
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<
            Self::OutboundProtocol,
            Self::OutboundOpenInfo,
            Self::OutEvent,
            Self::Error,
        >,
    > {
        let outbound_request = ConnectionHandlerEvent::OutboundSubstreamRequest {
            protocol: SubstreamProtocol::new(ReadyUpgrade::new(Self::PROTOCOL_NAME), ()),
        };
        for (&stream_id, stream) in &mut self.streams {
            match &mut stream.opening_state {
                None => {
                    stream.opening_state = Some(OpeningState::Requested);
                    return Poll::Ready(outbound_request);
                }
                Some(OpeningState::Requested) => {}
                Some(OpeningState::Negotiated { io }) => match stream.inner_state.poll(cx, io) {
                    Poll::Pending => (),
                    Poll::Ready(Err(err)) => {
                        if err.kind() == io::ErrorKind::UnexpectedEof {
                            if let StreamId::Outgoing(id) = stream_id {
                                log::warn!("requesting again");
                                stream.opening_state = Some(OpeningState::Requested);
                                self.last_outgoing_id.push_back(id);
                                return Poll::Ready(outbound_request);
                            } else {
                                return Poll::Ready(ConnectionHandlerEvent::Close(err));
                            }
                        } else {
                            return Poll::Ready(ConnectionHandlerEvent::Close(err));
                        }
                    }
                    Poll::Ready(Ok((header, bytes))) => {
                        let event = if let MessageHeader::Response(ResponseHeader { id: 0 }) =
                            header
                        {
                            let mut bytes = bytes.as_slice();
                            let menu = <ResponsePayload<<VersionedRpcMenuV1 as RpcMethod>::Response>>::binprot_read(
                                &mut bytes,
                            )
                            .unwrap()
                            .0
                            .unwrap()
                            .0
                            .into_iter()
                            .map(|(tag, version)| (tag.to_string_lossy(), version))
                            .collect();
                            Event::StreamNegotiated { stream_id, menu }
                        } else {
                            Event::Stream {
                                stream_id,
                                header,
                                bytes,
                            }
                        };
                        return Poll::Ready(ConnectionHandlerEvent::Custom(event));
                    }
                },
            }
        }

        self.waker = Some(cx.waker().clone());
        Poll::Pending
    }

    fn on_behaviour_event(&mut self, event: Self::InEvent) {
        match event {
            Command::Open { outgoing_stream_id } => {
                self.streams.insert(
                    StreamId::Outgoing(outgoing_stream_id),
                    Stream {
                        opening_state: None,
                        inner_state: state::Inner::new(self.menu.clone()),
                    },
                );
                self.last_outgoing_id.push_back(outgoing_stream_id);
            }
            Command::Send { stream_id, bytes } => {
                self.streams
                    .entry(stream_id)
                    .or_insert_with(|| {
                        if let StreamId::Outgoing(id) = stream_id {
                            self.last_outgoing_id.push_back(id);
                        }
                        Stream {
                            opening_state: None,
                            inner_state: state::Inner::new(self.menu.clone()),
                        }
                    })
                    .inner_state
                    .add(bytes);
            }
        }
        self.waker.as_ref().map(Waker::wake_by_ref);
    }

    fn on_connection_event(
        &mut self,
        event: ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
        match event {
            ConnectionEvent::FullyNegotiatedInbound(io) => self.add_stream(true, io.protocol),
            ConnectionEvent::FullyNegotiatedOutbound(io) => self.add_stream(false, io.protocol),
            _ => {}
        }
    }
}
