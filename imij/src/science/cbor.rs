//! Borrowed events for deterministic CBOR with text-keyed maps.

use alloc::vec::Vec;

use crate::{ByteReader, Error};

/// Per-reader limits for an expanded CBOR body.
#[derive(Clone, Copy, Debug)]
pub struct CborLimits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_items: usize,
    pub max_string_bytes: usize,
}

impl Default for CborLimits {
    fn default() -> Self {
        Self {
            max_bytes: crate::parse::chunks::MAX_PACKED_METADATA,
            max_depth: 32,
            max_items: 1_000_000,
            max_string_bytes: crate::parse::chunks::MAX_PACKED_METADATA,
        }
    }
}

/// One item or container boundary. Floats carry exact IEEE bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CborEvent<'a> {
    Unsigned(u64),
    /// Represents the integer `-1 - n`, including values below i64::MIN.
    Negative(u64),
    Bytes(&'a [u8]),
    Text(&'a str),
    Key(&'a str),
    Array(usize),
    Map(usize),
    End,
    Bool(bool),
    Null,
    Float16(u16),
    Float32(u32),
    Float64(u64),
}

struct Frame<'a> {
    remaining: usize,
    previous_key: Option<&'a [u8]>,
    map: bool,
}

/// Reads exactly one deterministic CBOR value. End events close containers.
/// Map keys are unique text strings in encoded byte order. Tags and other
/// simple values belong to application-specific readers.
pub struct CborReader<'a> {
    bytes: &'a [u8],
    input: ByteReader<'a>,
    stack: Vec<Frame<'a>>,
    limits: CborLimits,
    remaining_items: usize,
    started: bool,
    failed: bool,
}

impl<'a> CborReader<'a> {
    pub fn new(data: &'a [u8], limits: CborLimits) -> Result<Self, Error> {
        if data.len() > limits.max_bytes {
            return Err(Error::TooLarge);
        }
        Ok(Self {
            bytes: data,
            input: ByteReader::new(data),
            stack: Vec::new(),
            limits,
            remaining_items: limits.max_items,
            started: false,
            failed: false,
        })
    }

    /// Read one event. The caller can stop or check cancellation between
    /// events. Consume through `None` to validate the complete body.
    pub fn next_event(&mut self) -> Result<Option<CborEvent<'a>>, Error> {
        if self.failed {
            return Err(Error::Corrupt("failed CBOR reader"));
        }
        let result = self.next_inner();
        self.failed = result.is_err();
        result
    }

    fn next_inner(&mut self) -> Result<Option<CborEvent<'a>>, Error> {
        let mut key = false;
        if let Some(frame) = self.stack.last_mut() {
            if frame.remaining == 0 {
                self.stack.pop();
                return Ok(Some(CborEvent::End));
            }
            key = frame.map && frame.remaining % 2 == 0;
            frame.remaining -= 1;
        } else if self.started {
            return if self.input.remaining() == 0 {
                Ok(None)
            } else {
                Err(Error::Corrupt("trailing CBOR data"))
            };
        }
        self.started = true;
        self.remaining_items = self.remaining_items.checked_sub(1).ok_or(Error::TooLarge)?;
        let start = self.input.pos;
        let header = self.input.u8()?;
        let (major, extra) = (header >> 5, header & 31);
        if key && major != 3 {
            return Err(Error::Unsupported("CBOR map key is not text"));
        }
        let value = match major {
            0 => CborEvent::Unsigned(self.argument(extra)?),
            1 => CborEvent::Negative(self.argument(extra)?),
            2 | 3 => {
                let n = usize::try_from(self.argument(extra)?).map_err(|_| Error::TooLarge)?;
                if n > self.limits.max_string_bytes {
                    return Err(Error::TooLarge);
                }
                let bytes = self.input.bytes(n)?;
                if major == 2 {
                    CborEvent::Bytes(bytes)
                } else {
                    let text = core::str::from_utf8(bytes)
                        .map_err(|_| Error::Corrupt("CBOR text UTF-8"))?;
                    if key {
                        let encoded = &self.bytes[start..self.input.pos];
                        let frame = self.stack.last_mut().unwrap();
                        if frame
                            .previous_key
                            .is_some_and(|previous| previous >= encoded)
                        {
                            return Err(Error::Corrupt("CBOR map key order or duplicate"));
                        }
                        frame.previous_key = Some(encoded);
                        CborEvent::Key(text)
                    } else {
                        CborEvent::Text(text)
                    }
                }
            }
            4 | 5 => {
                let n = usize::try_from(self.argument(extra)?).map_err(|_| Error::TooLarge)?;
                let map = major == 5;
                let remaining = n
                    .checked_mul(if map { 2 } else { 1 })
                    .ok_or(Error::TooLarge)?;
                if self.stack.len() >= self.limits.max_depth || remaining > self.remaining_items {
                    return Err(Error::TooLarge);
                }
                if remaining > self.input.remaining() {
                    return Err(Error::Corrupt("truncated CBOR container"));
                }
                self.stack.try_reserve(1).map_err(|_| Error::TooLarge)?;
                self.stack.push(Frame {
                    remaining,
                    previous_key: None,
                    map,
                });
                if map {
                    CborEvent::Map(n)
                } else {
                    CborEvent::Array(n)
                }
            }
            7 => self.simple(extra)?,
            _ => return Err(Error::Unsupported("CBOR tag")),
        };
        Ok(Some(value))
    }

    fn argument(&mut self, extra: u8) -> Result<u64, Error> {
        let (value, minimum) = match extra {
            0..=23 => return Ok(u64::from(extra)),
            24 => (u64::from(self.input.u8()?), 24),
            25 => (
                u64::from(u16::from_be_bytes(self.input.bytes(2)?.try_into().unwrap())),
                256,
            ),
            26 => (
                u64::from(u32::from_be_bytes(self.input.bytes(4)?.try_into().unwrap())),
                65536,
            ),
            27 => (
                u64::from_be_bytes(self.input.bytes(8)?.try_into().unwrap()),
                1 << 32,
            ),
            _ => return Err(Error::Corrupt("indefinite or reserved CBOR argument")),
        };
        if value < minimum {
            return Err(Error::Corrupt("nonminimal CBOR argument"));
        }
        Ok(value)
    }

    fn simple(&mut self, extra: u8) -> Result<CborEvent<'a>, Error> {
        let nonminimal = Error::Corrupt("nonminimal CBOR float");
        Ok(match extra {
            20 | 21 => CborEvent::Bool(extra == 21),
            22 => CborEvent::Null,
            25 => {
                let bits = u16::from_be_bytes(self.input.bytes(2)?.try_into().unwrap());
                if bits != 0x7e00 && f32::from_bits(crate::mapping::f16_to_f32_bits(bits)).is_nan()
                {
                    return Err(nonminimal);
                }
                CborEvent::Float16(bits)
            }
            26 => {
                let bits = u32::from_be_bytes(self.input.bytes(4)?.try_into().unwrap());
                if f32::from_bits(bits).is_nan()
                    || crate::mapping::f32_to_f16_bits_exact(bits).is_some()
                {
                    return Err(nonminimal);
                }
                CborEvent::Float32(bits)
            }
            27 => {
                let bits = u64::from_be_bytes(self.input.bytes(8)?.try_into().unwrap());
                let value = f64::from_bits(bits);
                if value.is_nan() || ((value as f32) as f64).to_bits() == bits {
                    return Err(nonminimal);
                }
                CborEvent::Float64(bits)
            }
            _ => return Err(Error::Unsupported("CBOR simple value")),
        })
    }
}
