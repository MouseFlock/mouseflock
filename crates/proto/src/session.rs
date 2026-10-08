//! Per-connection protocol state: the `Hello` handshake on the control stream,
//! then checking every message against the agreed version.
//!
//! `transport` sends [`Session::local_hello`] as the first frame on the
//! control stream as soon as the connection is up, without waiting, and feeds
//! everything it receives through the `on_*` methods. Any error closes the
//! connection with [`SessionError::close_reason`].

use crate::datagram;
use crate::frame::Frame;
use crate::hello::{Hello, VersionMismatch, negotiate};
use crate::registry::{self, Channel, TypeInfo};
use crate::{CloseReason, ProtocolError};

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    Protocol(ProtocolError),
    VersionMismatch(VersionMismatch),
}

impl SessionError {
    /// The close reason to give the peer.
    pub const fn close_reason(&self) -> CloseReason {
        match self {
            Self::Protocol(_) => CloseReason::ProtocolError,
            Self::VersionMismatch(_) => CloseReason::VersionMismatch,
        }
    }
}

impl From<ProtocolError> for SessionError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

/// A message valid in the agreed version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message<'a> {
    pub info: &'static TypeInfo,
    pub payload: &'a [u8],
}

/// What a control-stream frame did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlEvent<'a> {
    /// The peer's `Hello` arrived and this version is now in use.
    Agreed(u16),
    Message(Message<'a>),
}

/// What to do with a frame from any other stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvent<'a> {
    Message(Message<'a>),
    /// No version agreed yet. Stop reading this stream and offer the frame
    /// again after [`ControlEvent::Agreed`]; QUIC flow control holds the peer.
    Wait,
}

/// What to do with a datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatagramEvent<'a> {
    Message(Message<'a>),
    /// Arrived before a version was agreed. Discard it.
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    AwaitingHello,
    Agreed(u16),
    Failed,
}

/// Protocol state for one peer connection.
#[derive(Debug, Clone)]
pub struct Session {
    local: Hello,
    state: State,
}

impl Session {
    pub const fn new(local: Hello) -> Self {
        Self {
            local,
            state: State::AwaitingHello,
        }
    }

    /// The `Hello` to send first on the control stream.
    pub const fn local_hello(&self) -> Hello {
        self.local
    }

    /// The agreed version, once the peer's `Hello` has arrived.
    pub const fn version(&self) -> Option<u16> {
        match self.state {
            State::Agreed(v) => Some(v),
            State::AwaitingHello | State::Failed => None,
        }
    }

    /// Handles a frame from the control stream.
    pub fn on_control_frame<'a>(
        &mut self,
        frame: Frame<'a>,
    ) -> Result<ControlEvent<'a>, SessionError> {
        let result = self.control_frame(frame);
        if result.is_err() {
            self.state = State::Failed;
        }
        result
    }

    fn control_frame<'a>(&mut self, frame: Frame<'a>) -> Result<ControlEvent<'a>, SessionError> {
        match self.state {
            State::Failed => Err(ProtocolError::SessionFailed.into()),
            State::AwaitingHello => {
                if frame.ty != registry::HELLO {
                    return Err(ProtocolError::ExpectedHello { ty: frame.ty }.into());
                }
                let remote = Hello::decode(frame.payload)?;
                let version =
                    negotiate(self.local, remote).map_err(SessionError::VersionMismatch)?;
                self.state = State::Agreed(version);
                Ok(ControlEvent::Agreed(version))
            }
            State::Agreed(version) => {
                if frame.ty == registry::HELLO {
                    return Err(ProtocolError::DuplicateHello.into());
                }
                Ok(ControlEvent::Message(message(
                    frame,
                    Channel::Stream,
                    version,
                )?))
            }
        }
    }

    /// Handles a frame from any stream other than the control stream. The only
    /// such streams are clipboard streams, so only their types are accepted.
    pub fn on_stream_frame<'a>(&self, frame: Frame<'a>) -> Result<StreamEvent<'a>, SessionError> {
        if frame.ty == registry::HELLO {
            return Err(ProtocolError::HelloOutsideControlStream.into());
        }
        match self.state {
            State::Failed => Err(ProtocolError::SessionFailed.into()),
            State::AwaitingHello => Ok(StreamEvent::Wait),
            State::Agreed(version) => Ok(StreamEvent::Message(message(
                frame,
                Channel::Transfer,
                version,
            )?)),
        }
    }

    /// Handles one received datagram.
    pub fn on_datagram<'a>(&self, bytes: &'a [u8]) -> Result<DatagramEvent<'a>, SessionError> {
        match self.state {
            State::Failed => Err(ProtocolError::SessionFailed.into()),
            State::AwaitingHello => Ok(DatagramEvent::Dropped),
            State::Agreed(version) => {
                let d = datagram::decode(bytes)?;
                let info = registry::check(d.ty, Channel::Datagram, version)?;
                Ok(DatagramEvent::Message(Message {
                    info,
                    payload: d.payload,
                }))
            }
        }
    }
}

fn message(frame: Frame<'_>, channel: Channel, version: u16) -> Result<Message<'_>, ProtocolError> {
    let info = registry::check(frame.ty, channel, version)?;
    Ok(Message {
        info,
        payload: frame.payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hello::Older;

    fn hello_frame(hello: &Hello) -> [u8; 4] {
        hello.payload()
    }

    fn agreed() -> Session {
        let mut s = Session::new(Hello::this_release());
        let payload = hello_frame(&Hello::this_release());
        let event = s.on_control_frame(Frame {
            ty: registry::HELLO,
            payload: &payload,
        });
        assert_eq!(event, Ok(ControlEvent::Agreed(2)));
        s
    }

    #[test]
    fn hello_agrees_version() {
        assert_eq!(agreed().version(), Some(2));
    }

    #[test]
    fn hello_from_a_version_1_peer_agrees_version_1() {
        let mut s = Session::new(Hello::this_release());
        let payload = hello_frame(&Hello { min: 1, max: 1 });
        let event = s.on_control_frame(Frame {
            ty: registry::HELLO,
            payload: &payload,
        });
        assert_eq!(event, Ok(ControlEvent::Agreed(1)));
        assert_eq!(s.version(), Some(1));
    }

    #[test]
    fn message_before_hello_is_rejected() {
        let mut s = Session::new(Hello::this_release());
        let result = s.on_control_frame(Frame {
            ty: 0x0042,
            payload: &[],
        });
        assert_eq!(
            result,
            Err(SessionError::Protocol(ProtocolError::ExpectedHello {
                ty: 0x0042
            }))
        );
        assert_eq!(
            result.unwrap_err().close_reason(),
            CloseReason::ProtocolError
        );
    }

    #[test]
    fn second_hello_is_rejected() {
        let mut s = agreed();
        let payload = hello_frame(&Hello::this_release());
        assert_eq!(
            s.on_control_frame(Frame {
                ty: registry::HELLO,
                payload: &payload
            }),
            Err(SessionError::Protocol(ProtocolError::DuplicateHello))
        );
    }

    #[test]
    fn invalid_version_range_is_rejected() {
        let mut s = Session::new(Hello::this_release());
        assert_eq!(
            s.on_control_frame(Frame {
                ty: registry::HELLO,
                payload: &[0x03, 0x00, 0x02, 0x00]
            }),
            Err(SessionError::Protocol(ProtocolError::InvalidVersionRange {
                min: 3,
                max: 2
            }))
        );
    }

    #[test]
    fn version_mismatch_closes_with_mismatch_reason() {
        let mut s = Session::new(Hello { min: 4, max: 5 });
        let result = s.on_control_frame(Frame {
            ty: registry::HELLO,
            payload: &[0x01, 0x00, 0x02, 0x00],
        });
        let Err(err @ SessionError::VersionMismatch(mismatch)) = result else {
            panic!("expected mismatch, got {result:?}")
        };
        assert_eq!(mismatch.older, Older::OtherPeer);
        assert_eq!(err.close_reason(), CloseReason::VersionMismatch);
    }

    #[test]
    fn unknown_type_after_agreement_is_rejected() {
        let mut s = agreed();
        assert_eq!(
            s.on_control_frame(Frame {
                ty: 0x0042,
                payload: &[]
            }),
            Err(SessionError::Protocol(ProtocolError::UnknownType {
                ty: 0x0042
            }))
        );
    }

    #[test]
    fn failed_session_rejects_everything() {
        let mut s = Session::new(Hello::this_release());
        let _ = s.on_control_frame(Frame {
            ty: 0x0042,
            payload: &[],
        });
        let payload = hello_frame(&Hello::this_release());
        assert_eq!(
            s.on_control_frame(Frame {
                ty: registry::HELLO,
                payload: &payload
            }),
            Err(SessionError::Protocol(ProtocolError::SessionFailed))
        );
        assert!(s.on_datagram(&[0x02, 0x00]).is_err());
    }

    #[test]
    fn hello_on_other_stream_is_rejected_before_and_after_agreement() {
        let payload = hello_frame(&Hello::this_release());
        let frame = Frame {
            ty: registry::HELLO,
            payload: &payload,
        };
        let expected = Err(SessionError::Protocol(
            ProtocolError::HelloOutsideControlStream,
        ));
        assert_eq!(
            Session::new(Hello::this_release()).on_stream_frame(frame),
            expected
        );
        assert_eq!(agreed().on_stream_frame(frame), expected);
    }

    #[test]
    fn stream_frame_before_hello_waits() {
        let s = Session::new(Hello::this_release());
        assert_eq!(
            s.on_stream_frame(Frame {
                ty: 0x0042,
                payload: &[]
            }),
            Ok(StreamEvent::Wait)
        );
    }

    #[test]
    fn held_stream_frame_is_checked_after_agreement() {
        let frame = Frame {
            ty: 0x0042,
            payload: &[],
        };
        assert_eq!(
            agreed().on_stream_frame(frame),
            Err(SessionError::Protocol(ProtocolError::UnknownType {
                ty: 0x0042
            }))
        );
    }

    #[test]
    fn datagram_before_hello_is_dropped() {
        let s = Session::new(Hello::this_release());
        assert_eq!(s.on_datagram(&[0x01]), Ok(DatagramEvent::Dropped));
        assert_eq!(s.on_datagram(&[0x01, 0x00]), Ok(DatagramEvent::Dropped));
    }

    #[test]
    fn hello_in_datagram_after_agreement_is_rejected() {
        assert_eq!(
            agreed().on_datagram(&[0x01, 0x00, 0x01, 0x00, 0x01, 0x00]),
            Err(SessionError::Protocol(ProtocolError::WrongChannel {
                ty: registry::HELLO
            }))
        );
    }

    #[test]
    fn malformed_datagram_after_agreement_is_rejected() {
        assert_eq!(
            agreed().on_datagram(&[0x01]),
            Err(SessionError::Protocol(ProtocolError::DatagramTooShort {
                len: 1
            }))
        );
    }
}
