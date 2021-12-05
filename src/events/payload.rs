use crate::prelude::IPCResult;
use byteorder::{BigEndian, ReadBytesExt};
use std::io::Read;

#[cfg(feature = "serialize")]
pub use super::payload_serializer::*;

/// Trait to convert event data into sending bytes
/// It is implemented for all types that implement Serialize
pub trait EventSendPayload {
    fn to_payload_bytes(self) -> IPCResult<Vec<u8>>;
}

/// Trait to get the event data from receiving bytes.
/// It is implemented for all types that are DeserializeOwned
pub trait EventReceivePayload: Sized {
    fn from_payload_bytes<R: Read>(reader: R) -> IPCResult<Self>;
}

/// A payload wrapper type for sending bytes directly without
/// serializing them
#[derive(Clone)]
pub struct BytePayload {
    bytes: Vec<u8>,
}

impl BytePayload {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Returns the bytes of the payload
    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl EventSendPayload for BytePayload {
    fn to_payload_bytes(self) -> IPCResult<Vec<u8>> {
        Ok(self.bytes)
    }
}

impl EventReceivePayload for BytePayload {
    fn from_payload_bytes<R: Read>(mut reader: R) -> IPCResult<Self> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;

        Ok(Self::new(buf))
    }
}

/// A payload wrapper that allows storing two different payloads
/// independent from each other. For example one payload can be
/// a payload serialized by serde while the other is a raw byte
/// payload
pub struct TandemPayload<P1, P2> {
    load1: P1,
    load2: P2,
}

impl<P1, P2> TandemPayload<P1, P2> {
    pub fn new(load1: P1, load2: P2) -> Self {
        Self { load1, load2 }
    }

    /// Returns both payload stored in the tandem payload
    pub fn into_inner(self) -> (P1, P2) {
        (self.load1, self.load2)
    }
}

impl<P1, P2> EventSendPayload for TandemPayload<P1, P2>
where
    P1: EventSendPayload,
    P2: EventSendPayload,
{
    fn to_payload_bytes(self) -> IPCResult<Vec<u8>> {
        let mut p1_bytes = self.load1.to_payload_bytes()?;
        let mut p2_bytes = self.load2.to_payload_bytes()?;

        let mut p1_length_bytes = (p1_bytes.len() as u64).to_be_bytes().to_vec();
        let mut p2_length_bytes = (p2_bytes.len() as u64).to_be_bytes().to_vec();

        let mut bytes = Vec::new();
        bytes.append(&mut p1_length_bytes);
        bytes.append(&mut p1_bytes);
        bytes.append(&mut p2_length_bytes);
        bytes.append(&mut p2_bytes);

        Ok(bytes)
    }
}

impl<P1, P2> EventReceivePayload for TandemPayload<P1, P2>
where
    P1: EventReceivePayload,
    P2: EventReceivePayload,
{
    fn from_payload_bytes<R: Read>(mut reader: R) -> IPCResult<Self> {
        let p1_length = reader.read_u64::<BigEndian>()?;
        let mut load1_bytes = vec![0u8; p1_length as usize];
        reader.read_exact(&mut load1_bytes)?;

        let p2_length = reader.read_u64::<BigEndian>()?;
        let mut load2_bytes = vec![0u8; p2_length as usize];
        reader.read_exact(&mut load2_bytes)?;

        Ok(Self {
            load1: P1::from_payload_bytes(load1_bytes.as_slice())?,
            load2: P2::from_payload_bytes(load2_bytes.as_slice())?,
        })
    }
}

impl EventSendPayload for () {
    fn to_payload_bytes(self) -> IPCResult<Vec<u8>> {
        Ok(vec![])
    }
}

#[cfg(feature = "serialize")]
mod serde_payload {
    use super::DynamicSerializer;
    use crate::payload::EventReceivePayload;
    use crate::prelude::{EventSendPayload, IPCResult};
    use byteorder::ReadBytesExt;
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use std::io::Read;

    /// A payload representing a payload storing serde serialized data
    pub struct SerdePayload<T> {
        data: T,
        serializer: DynamicSerializer,
    }

    impl<T> SerdePayload<T> {
        /// Creates a new serde payload with a specified serializer
        pub fn new(serializer: DynamicSerializer, data: T) -> Self {
            Self { serializer, data }
        }

        pub fn data(self) -> T {
            self.data
        }
    }

    impl<T> Clone for SerdePayload<T>
    where
        T: Clone,
    {
        fn clone(&self) -> Self {
            Self {
                serializer: self.serializer.clone(),
                data: self.data.clone(),
            }
        }
    }

    impl<T> EventSendPayload for SerdePayload<T>
    where
        T: Serialize,
    {
        fn to_payload_bytes(self) -> IPCResult<Vec<u8>> {
            let mut buf = Vec::new();
            let mut data_bytes = self.serializer.serialize(self.data)?;
            let format_id = self.serializer as u8;
            buf.push(format_id);
            buf.append(&mut data_bytes);

            Ok(buf)
        }
    }

    impl<T> EventReceivePayload for SerdePayload<T>
    where
        T: DeserializeOwned,
    {
        fn from_payload_bytes<R: Read>(mut reader: R) -> IPCResult<Self> {
            let format_id = reader.read_u8()?;
            let serializer = DynamicSerializer::from_primitive(format_id as usize)?;
            let data = serializer.deserialize(reader)?;

            Ok(Self { serializer, data })
        }
    }
}

#[cfg(feature = "serialize")]
pub use serde_payload::*;
