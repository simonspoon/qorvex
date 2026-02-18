//! Binary wire protocol for communication between the Rust host and Swift agent.
//!
//! This module defines the binary protocol used over TCP for communication with
//! the native Swift accessibility agent.
//!
//! # Packet Structure (Little Endian)
//!
//! ```text
//! [Header: 4 bytes LE u32 len] [OpCode: 1 byte] [Payload: variable]
//! ```
//!
//! The `len` field encodes the total length of the opcode + payload (NOT including
//! the 4-byte header itself).
//!
//! # String Encoding
//!
//! Strings are length-prefixed: a `u32` LE byte count followed by UTF-8 bytes.
//!
//! # Optional Values
//!
//! Optional fields use a `u8` presence flag (`0` = None, `1` = Some) followed by
//! the value when present.
//!
//! # Example
//!
//! ```
//! use qorvex_core::protocol::{Request, Response, encode_request, decode_request};
//!
//! let req = Request::TapCoord { x: 100, y: 200 };
//! let wire = encode_request(&req);
//!
//! // Skip the 4-byte length header to decode
//! let decoded = decode_request(&wire[4..]).unwrap();
//! ```

use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during protocol encoding or decoding.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    /// The opcode byte does not correspond to any known operation.
    #[error("invalid opcode: 0x{0:02X}")]
    InvalidOpCode(u8),

    /// The buffer does not contain enough bytes for the expected data.
    #[error("insufficient data in buffer")]
    InsufficientData,

    /// A string field contains invalid UTF-8.
    #[error("invalid UTF-8 in string field")]
    Utf8Error,

    /// The payload structure is invalid for the given opcode.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
}

// ---------------------------------------------------------------------------
// OpCode
// ---------------------------------------------------------------------------

/// On-the-wire operation codes.
///
/// Each request or response starts with a single-byte opcode that identifies the
/// message type and determines how the remaining payload is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    /// Keep-alive ping (no payload).
    Heartbeat = 0x01,
    /// Tap at absolute coordinates (i32 x, i32 y).
    TapCoord = 0x02,
    /// Tap element by accessibility identifier (length-prefixed string).
    TapElement = 0x03,
    /// Tap element by accessibility label (length-prefixed string).
    TapByLabel = 0x04,
    /// Tap with element type filter (selector string + by_label bool + type string).
    TapWithType = 0x05,
    /// Type text via keyboard (length-prefixed string).
    TypeText = 0x06,
    /// Swipe gesture (i32 start_x, start_y, end_x, end_y + optional f64 duration).
    Swipe = 0x07,
    /// Get current value of an element (selector + by_label + optional type).
    GetValue = 0x08,
    /// Long press at coordinates (i32 x, i32 y, f64 duration).
    LongPress = 0x09,
    /// Request a full accessibility tree dump (no payload).
    DumpTree = 0x10,
    /// Request a screenshot capture (no payload).
    Screenshot = 0x11,
    /// Set the target application for accessibility queries (length-prefixed string).
    SetTarget = 0x12,
    /// Find a single element matching the selector (selector + by_label + optional type).
    FindElement = 0x13,
    /// Error message from the agent (length-prefixed string).
    Error = 0x99,
    /// Generic response (response-type byte + variable data).
    Response = 0xA0,
}

impl OpCode {
    /// Try to convert a raw byte into an `OpCode`.
    pub fn from_u8(byte: u8) -> Result<Self, ProtocolError> {
        match byte {
            0x01 => Ok(OpCode::Heartbeat),
            0x02 => Ok(OpCode::TapCoord),
            0x03 => Ok(OpCode::TapElement),
            0x04 => Ok(OpCode::TapByLabel),
            0x05 => Ok(OpCode::TapWithType),
            0x06 => Ok(OpCode::TypeText),
            0x07 => Ok(OpCode::Swipe),
            0x08 => Ok(OpCode::GetValue),
            0x09 => Ok(OpCode::LongPress),
            0x10 => Ok(OpCode::DumpTree),
            0x11 => Ok(OpCode::Screenshot),
            0x12 => Ok(OpCode::SetTarget),
            0x13 => Ok(OpCode::FindElement),
            0x99 => Ok(OpCode::Error),
            0xA0 => Ok(OpCode::Response),
            other => Err(ProtocolError::InvalidOpCode(other)),
        }
    }
}

// ---------------------------------------------------------------------------
// Request / Response enums
// ---------------------------------------------------------------------------

/// A high-level typed request from the Rust host to the Swift agent.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Keep-alive heartbeat.
    Heartbeat,
    /// Tap at absolute screen coordinates.
    TapCoord { x: i32, y: i32 },
    /// Tap an element by its accessibility identifier.
    TapElement { selector: String },
    /// Tap an element by its accessibility label.
    TapByLabel { label: String },
    /// Tap an element with an explicit type filter.
    TapWithType {
        selector: String,
        by_label: bool,
        element_type: String,
    },
    /// Send keyboard input text.
    TypeText { text: String },
    /// Perform a swipe gesture between two points.
    Swipe {
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        duration: Option<f64>,
    },
    /// Retrieve the current value of a UI element.
    GetValue {
        selector: String,
        by_label: bool,
        element_type: Option<String>,
    },
    /// Perform a long press at specific screen coordinates.
    LongPress { x: i32, y: i32, duration: f64 },
    /// Request the full accessibility tree.
    DumpTree,
    /// Request a screenshot.
    Screenshot,
    /// Set the target application bundle ID for accessibility queries.
    SetTarget { bundle_id: String },
    /// Find a single element matching the selector.
    FindElement {
        selector: String,
        by_label: bool,
        element_type: Option<String>,
    },
}

impl Request {
    /// Returns a short, static name for this request type suitable for use in
    /// tracing span metadata. Avoids Debug-formatting large enum payloads.
    pub fn opcode_name(&self) -> &'static str {
        match self {
            Request::Heartbeat => "heartbeat",
            Request::TapCoord { .. } => "tap_coord",
            Request::TapElement { .. } => "tap_element",
            Request::TapByLabel { .. } => "tap_by_label",
            Request::TapWithType { .. } => "tap_with_type",
            Request::TypeText { .. } => "type_text",
            Request::Swipe { .. } => "swipe",
            Request::GetValue { .. } => "get_value",
            Request::LongPress { .. } => "long_press",
            Request::DumpTree => "dump_tree",
            Request::Screenshot => "screenshot",
            Request::SetTarget { .. } => "set_target",
            Request::FindElement { .. } => "find_element",
        }
    }
}

/// Response sub-type byte used inside the `Response` opcode payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum ResponseType {
    Ok = 0x00,
    Error = 0x01,
    Tree = 0x02,
    Screenshot = 0x03,
    Value = 0x04,
    Element = 0x05,
}

impl ResponseType {
    fn from_u8(byte: u8) -> Result<Self, ProtocolError> {
        match byte {
            0x00 => Ok(ResponseType::Ok),
            0x01 => Ok(ResponseType::Error),
            0x02 => Ok(ResponseType::Tree),
            0x03 => Ok(ResponseType::Screenshot),
            0x04 => Ok(ResponseType::Value),
            0x05 => Ok(ResponseType::Element),
            other => Err(ProtocolError::InvalidPayload(format!(
                "unknown response type: 0x{other:02X}"
            ))),
        }
    }
}

/// A typed response from the Swift agent to the Rust host.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// The operation completed successfully with no additional data.
    Ok,
    /// The operation failed.
    Error { message: String },
    /// A JSON-encoded accessibility tree.
    Tree { json: String },
    /// Raw screenshot image bytes (PNG or JPEG).
    Screenshot { data: Vec<u8> },
    /// The current value of a UI element, if available.
    Value { value: Option<String> },
    /// A JSON-encoded single element result.
    Element { json: String },
}

// ---------------------------------------------------------------------------
// Low-level payload helpers
// ---------------------------------------------------------------------------

/// Write a length-prefixed UTF-8 string into `buf`.
///
/// Format: `[u32 LE byte_count] [UTF-8 bytes]`
fn write_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
}

/// Write raw bytes with a u32 LE length prefix into `buf`.
fn write_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(data);
}

/// Write an optional string into `buf`.
///
/// Format: `[u8 flag]` where flag=0 means None, flag=1 means Some followed by
/// a length-prefixed string.
fn write_optional_string(buf: &mut Vec<u8>, opt: &Option<String>) {
    match opt {
        None => buf.push(0u8),
        Some(s) => {
            buf.push(1u8);
            write_string(buf, s);
        }
    }
}

/// Write a bool as a single `u8` (0 = false, 1 = true).
fn write_bool(buf: &mut Vec<u8>, v: bool) {
    buf.push(if v { 1u8 } else { 0u8 });
}

/// A cursor over a byte slice for sequential reads.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        if self.remaining() < 1 {
            return Err(ProtocolError::InsufficientData);
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_i32(&mut self) -> Result<i32, ProtocolError> {
        if self.remaining() < 4 {
            return Err(ProtocolError::InsufficientData);
        }
        let bytes: [u8; 4] = self.data[self.pos..self.pos + 4]
            .try_into()
            .map_err(|_| ProtocolError::InsufficientData)?;
        self.pos += 4;
        Ok(i32::from_le_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32, ProtocolError> {
        if self.remaining() < 4 {
            return Err(ProtocolError::InsufficientData);
        }
        let bytes: [u8; 4] = self.data[self.pos..self.pos + 4]
            .try_into()
            .map_err(|_| ProtocolError::InsufficientData)?;
        self.pos += 4;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_f64(&mut self) -> Result<f64, ProtocolError> {
        if self.remaining() < 8 {
            return Err(ProtocolError::InsufficientData);
        }
        let bytes: [u8; 8] = self.data[self.pos..self.pos + 8]
            .try_into()
            .map_err(|_| ProtocolError::InsufficientData)?;
        self.pos += 8;
        Ok(f64::from_le_bytes(bytes))
    }

    fn read_bool(&mut self) -> Result<bool, ProtocolError> {
        Ok(self.read_u8()? != 0)
    }

    /// Read a length-prefixed UTF-8 string.
    fn read_string(&mut self) -> Result<String, ProtocolError> {
        let len = self.read_u32()? as usize;
        if self.remaining() < len {
            return Err(ProtocolError::InsufficientData);
        }
        let s = std::str::from_utf8(&self.data[self.pos..self.pos + len])
            .map_err(|_| ProtocolError::Utf8Error)?;
        self.pos += len;
        Ok(s.to_owned())
    }

    /// Read a length-prefixed raw byte slice.
    fn read_bytes(&mut self) -> Result<Vec<u8>, ProtocolError> {
        let len = self.read_u32()? as usize;
        if self.remaining() < len {
            return Err(ProtocolError::InsufficientData);
        }
        let v = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        Ok(v)
    }

    /// Read an optional length-prefixed string.
    fn read_optional_string(&mut self) -> Result<Option<String>, ProtocolError> {
        let flag = self.read_u8()?;
        if flag == 0 {
            Ok(None)
        } else {
            Ok(Some(self.read_string()?))
        }
    }
}

// ---------------------------------------------------------------------------
// Frame helpers
// ---------------------------------------------------------------------------

/// Wrap a payload (opcode + data) with the 4-byte LE length header.
///
/// The returned buffer contains `[u32 LE length][payload]` where `length` equals
/// `payload.len()`.
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Read the payload length from a 4-byte LE header.
pub fn read_frame_length(header: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*header)
}

// ---------------------------------------------------------------------------
// Encode request
// ---------------------------------------------------------------------------

/// Encode a [`Request`] into wire format including the 4-byte length header.
///
/// The returned bytes are ready to be written directly to a TCP stream.
pub fn encode_request(req: &Request) -> Vec<u8> {
    let mut payload = Vec::new();

    match req {
        Request::Heartbeat => {
            payload.push(OpCode::Heartbeat as u8);
        }
        Request::TapCoord { x, y } => {
            payload.push(OpCode::TapCoord as u8);
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
        }
        Request::TapElement { selector } => {
            payload.push(OpCode::TapElement as u8);
            write_string(&mut payload, selector);
        }
        Request::TapByLabel { label } => {
            payload.push(OpCode::TapByLabel as u8);
            write_string(&mut payload, label);
        }
        Request::TapWithType {
            selector,
            by_label,
            element_type,
        } => {
            payload.push(OpCode::TapWithType as u8);
            write_string(&mut payload, selector);
            write_bool(&mut payload, *by_label);
            write_string(&mut payload, element_type);
        }
        Request::TypeText { text } => {
            payload.push(OpCode::TypeText as u8);
            write_string(&mut payload, text);
        }
        Request::Swipe {
            start_x,
            start_y,
            end_x,
            end_y,
            duration,
        } => {
            payload.push(OpCode::Swipe as u8);
            payload.extend_from_slice(&start_x.to_le_bytes());
            payload.extend_from_slice(&start_y.to_le_bytes());
            payload.extend_from_slice(&end_x.to_le_bytes());
            payload.extend_from_slice(&end_y.to_le_bytes());
            match duration {
                None => payload.push(0u8),
                Some(d) => {
                    payload.push(1u8);
                    payload.extend_from_slice(&d.to_le_bytes());
                }
            }
        }
        Request::GetValue {
            selector,
            by_label,
            element_type,
        } => {
            payload.push(OpCode::GetValue as u8);
            write_string(&mut payload, selector);
            write_bool(&mut payload, *by_label);
            write_optional_string(&mut payload, element_type);
        }
        Request::LongPress { x, y, duration } => {
            payload.push(OpCode::LongPress as u8);
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
            payload.extend_from_slice(&duration.to_le_bytes());
        }
        Request::DumpTree => {
            payload.push(OpCode::DumpTree as u8);
        }
        Request::Screenshot => {
            payload.push(OpCode::Screenshot as u8);
        }
        Request::SetTarget { bundle_id } => {
            payload.push(OpCode::SetTarget as u8);
            write_string(&mut payload, bundle_id);
        }
        Request::FindElement {
            selector,
            by_label,
            element_type,
        } => {
            payload.push(OpCode::FindElement as u8);
            write_string(&mut payload, selector);
            write_bool(&mut payload, *by_label);
            write_optional_string(&mut payload, element_type);
        }
    }

    encode_frame(&payload)
}

// ---------------------------------------------------------------------------
// Decode request
// ---------------------------------------------------------------------------

/// Decode wire bytes (opcode + payload, **after** the 4-byte length header) into
/// a [`Request`].
///
/// Pass the slice starting at the opcode byte; do **not** include the length header.
pub fn decode_request(data: &[u8]) -> Result<Request, ProtocolError> {
    let mut cur = Cursor::new(data);
    let opcode = OpCode::from_u8(cur.read_u8()?)?;

    match opcode {
        OpCode::Heartbeat => Ok(Request::Heartbeat),

        OpCode::TapCoord => {
            let x = cur.read_i32()?;
            let y = cur.read_i32()?;
            Ok(Request::TapCoord { x, y })
        }

        OpCode::TapElement => {
            let selector = cur.read_string()?;
            Ok(Request::TapElement { selector })
        }

        OpCode::TapByLabel => {
            let label = cur.read_string()?;
            Ok(Request::TapByLabel { label })
        }

        OpCode::TapWithType => {
            let selector = cur.read_string()?;
            let by_label = cur.read_bool()?;
            let element_type = cur.read_string()?;
            Ok(Request::TapWithType {
                selector,
                by_label,
                element_type,
            })
        }

        OpCode::TypeText => {
            let text = cur.read_string()?;
            Ok(Request::TypeText { text })
        }

        OpCode::Swipe => {
            let start_x = cur.read_i32()?;
            let start_y = cur.read_i32()?;
            let end_x = cur.read_i32()?;
            let end_y = cur.read_i32()?;
            let has_duration = cur.read_bool()?;
            let duration = if has_duration {
                Some(cur.read_f64()?)
            } else {
                None
            };
            Ok(Request::Swipe {
                start_x,
                start_y,
                end_x,
                end_y,
                duration,
            })
        }

        OpCode::GetValue => {
            let selector = cur.read_string()?;
            let by_label = cur.read_bool()?;
            let element_type = cur.read_optional_string()?;
            Ok(Request::GetValue {
                selector,
                by_label,
                element_type,
            })
        }

        OpCode::LongPress => {
            let x = cur.read_i32()?;
            let y = cur.read_i32()?;
            let duration = cur.read_f64()?;
            Ok(Request::LongPress { x, y, duration })
        }

        OpCode::DumpTree => Ok(Request::DumpTree),

        OpCode::Screenshot => Ok(Request::Screenshot),

        OpCode::SetTarget => {
            let bundle_id = cur.read_string()?;
            Ok(Request::SetTarget { bundle_id })
        }

        OpCode::FindElement => {
            let selector = cur.read_string()?;
            let by_label = cur.read_bool()?;
            let element_type = cur.read_optional_string()?;
            Ok(Request::FindElement {
                selector,
                by_label,
                element_type,
            })
        }

        OpCode::Error | OpCode::Response => Err(ProtocolError::InvalidPayload(format!(
            "opcode 0x{:02X} is not a valid request opcode",
            opcode as u8
        ))),
    }
}

// ---------------------------------------------------------------------------
// Encode response
// ---------------------------------------------------------------------------

/// Encode a [`Response`] into wire format including the 4-byte length header.
pub fn encode_response(resp: &Response) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.push(OpCode::Response as u8);

    match resp {
        Response::Ok => {
            payload.push(ResponseType::Ok as u8);
        }
        Response::Error { message } => {
            payload.push(ResponseType::Error as u8);
            write_string(&mut payload, message);
        }
        Response::Tree { json } => {
            payload.push(ResponseType::Tree as u8);
            write_string(&mut payload, json);
        }
        Response::Screenshot { data } => {
            payload.push(ResponseType::Screenshot as u8);
            write_bytes(&mut payload, data);
        }
        Response::Value { value } => {
            payload.push(ResponseType::Value as u8);
            write_optional_string(&mut payload, value);
        }
        Response::Element { json } => {
            payload.push(ResponseType::Element as u8);
            write_string(&mut payload, json);
        }
    }

    encode_frame(&payload)
}

// ---------------------------------------------------------------------------
// Decode response
// ---------------------------------------------------------------------------

/// Decode wire bytes (opcode + payload, **after** the 4-byte length header) into
/// a [`Response`].
///
/// The first byte must be the `Response` opcode (`0xA0`), followed by a
/// response-type discriminator and the type-specific payload.
pub fn decode_response(data: &[u8]) -> Result<Response, ProtocolError> {
    let mut cur = Cursor::new(data);
    let opcode = OpCode::from_u8(cur.read_u8()?)?;

    match opcode {
        OpCode::Response => {
            let resp_type = ResponseType::from_u8(cur.read_u8()?)?;
            match resp_type {
                ResponseType::Ok => Ok(Response::Ok),
                ResponseType::Error => {
                    let message = cur.read_string()?;
                    Ok(Response::Error { message })
                }
                ResponseType::Tree => {
                    let json = cur.read_string()?;
                    Ok(Response::Tree { json })
                }
                ResponseType::Screenshot => {
                    let data = cur.read_bytes()?;
                    Ok(Response::Screenshot { data })
                }
                ResponseType::Value => {
                    let value = cur.read_optional_string()?;
                    Ok(Response::Value { value })
                }
                ResponseType::Element => {
                    let json = cur.read_string()?;
                    Ok(Response::Element { json })
                }
            }
        }

        OpCode::Error => {
            // The agent may also send a bare Error opcode.
            let message = cur.read_string()?;
            Ok(Response::Error { message })
        }

        _ => Err(ProtocolError::InvalidPayload(format!(
            "opcode 0x{:02X} is not a valid response opcode",
            opcode as u8
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- helper: round-trip request -----------------------------------------

    fn round_trip_request(req: &Request) {
        let wire = encode_request(req);
        // The first 4 bytes are the length header.
        let len = read_frame_length(wire[..4].try_into().unwrap()) as usize;
        assert_eq!(len, wire.len() - 4);
        let decoded = decode_request(&wire[4..]).expect("decode_request failed");
        assert_eq!(&decoded, req);
    }

    // -- helper: round-trip response ----------------------------------------

    fn round_trip_response(resp: &Response) {
        let wire = encode_response(resp);
        let len = read_frame_length(wire[..4].try_into().unwrap()) as usize;
        assert_eq!(len, wire.len() - 4);
        let decoded = decode_response(&wire[4..]).expect("decode_response failed");
        assert_eq!(&decoded, resp);
    }

    // -- Request round-trips ------------------------------------------------

    #[test]
    fn request_heartbeat() {
        round_trip_request(&Request::Heartbeat);
    }

    #[test]
    fn request_tap_coord() {
        round_trip_request(&Request::TapCoord { x: 100, y: -42 });
    }

    #[test]
    fn request_tap_coord_zero() {
        round_trip_request(&Request::TapCoord { x: 0, y: 0 });
    }

    #[test]
    fn request_tap_element() {
        round_trip_request(&Request::TapElement {
            selector: "login-button".into(),
        });
    }

    #[test]
    fn request_tap_element_empty_selector() {
        round_trip_request(&Request::TapElement {
            selector: String::new(),
        });
    }

    #[test]
    fn request_tap_by_label() {
        round_trip_request(&Request::TapByLabel {
            label: "Sign In".into(),
        });
    }

    #[test]
    fn request_tap_with_type_by_id() {
        round_trip_request(&Request::TapWithType {
            selector: "submit-btn".into(),
            by_label: false,
            element_type: "Button".into(),
        });
    }

    #[test]
    fn request_tap_with_type_by_label() {
        round_trip_request(&Request::TapWithType {
            selector: "Next".into(),
            by_label: true,
            element_type: "Button".into(),
        });
    }

    #[test]
    fn request_type_text() {
        round_trip_request(&Request::TypeText {
            text: "hello world".into(),
        });
    }

    #[test]
    fn request_type_text_unicode() {
        round_trip_request(&Request::TypeText {
            text: "cafe\u{0301} \u{1F600}".into(),
        });
    }

    #[test]
    fn request_swipe_no_duration() {
        round_trip_request(&Request::Swipe {
            start_x: 0,
            start_y: 100,
            end_x: 0,
            end_y: 500,
            duration: None,
        });
    }

    #[test]
    fn request_swipe_with_duration() {
        round_trip_request(&Request::Swipe {
            start_x: 50,
            start_y: 800,
            end_x: 50,
            end_y: 200,
            duration: Some(0.5),
        });
    }

    #[test]
    fn request_get_value_no_type() {
        round_trip_request(&Request::GetValue {
            selector: "email-field".into(),
            by_label: false,
            element_type: None,
        });
    }

    #[test]
    fn request_get_value_with_type() {
        round_trip_request(&Request::GetValue {
            selector: "Email".into(),
            by_label: true,
            element_type: Some("TextField".into()),
        });
    }

    #[test]
    fn request_long_press() {
        round_trip_request(&Request::LongPress {
            x: 150,
            y: 300,
            duration: 1.5,
        });
    }

    #[test]
    fn request_long_press_zero_duration() {
        round_trip_request(&Request::LongPress {
            x: 0,
            y: 0,
            duration: 0.0,
        });
    }

    #[test]
    fn request_dump_tree() {
        round_trip_request(&Request::DumpTree);
    }

    #[test]
    fn request_screenshot() {
        round_trip_request(&Request::Screenshot);
    }

    #[test]
    fn request_set_target() {
        round_trip_request(&Request::SetTarget {
            bundle_id: "com.example.myapp".into(),
        });
    }

    // -- Response round-trips -----------------------------------------------

    #[test]
    fn response_ok() {
        round_trip_response(&Response::Ok);
    }

    #[test]
    fn response_error() {
        round_trip_response(&Response::Error {
            message: "element not found".into(),
        });
    }

    #[test]
    fn response_tree() {
        round_trip_response(&Response::Tree {
            json: r#"{"type":"View","children":[]}"#.into(),
        });
    }

    #[test]
    fn response_screenshot() {
        round_trip_response(&Response::Screenshot {
            data: vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        });
    }

    #[test]
    fn response_screenshot_empty() {
        round_trip_response(&Response::Screenshot { data: vec![] });
    }

    #[test]
    fn response_value_some() {
        round_trip_response(&Response::Value {
            value: Some("hello@example.com".into()),
        });
    }

    #[test]
    fn response_value_none() {
        round_trip_response(&Response::Value { value: None });
    }

    // -- Error cases --------------------------------------------------------

    #[test]
    fn decode_request_empty_input() {
        let result = decode_request(&[]);
        assert_eq!(result, Err(ProtocolError::InsufficientData));
    }

    #[test]
    fn decode_request_invalid_opcode() {
        let result = decode_request(&[0xFF]);
        assert_eq!(result, Err(ProtocolError::InvalidOpCode(0xFF)));
    }

    #[test]
    fn decode_request_truncated_payload() {
        // TapCoord needs 8 bytes after opcode but we only give 4.
        let result = decode_request(&[OpCode::TapCoord as u8, 0, 0, 0, 0]);
        assert_eq!(result, Err(ProtocolError::InsufficientData));
    }

    #[test]
    fn decode_response_invalid_opcode() {
        // An opcode that is a request opcode should fail when decoded as response.
        let result = decode_response(&[OpCode::TapCoord as u8]);
        assert!(result.is_err());
    }

    #[test]
    fn decode_response_invalid_response_type() {
        let result = decode_response(&[OpCode::Response as u8, 0xFF]);
        assert!(result.is_err());
    }

    #[test]
    fn decode_response_bare_error_opcode() {
        // The agent can send a bare Error opcode with a message.
        let mut wire = vec![OpCode::Error as u8];
        let msg = "something broke";
        wire.extend_from_slice(&(msg.len() as u32).to_le_bytes());
        wire.extend_from_slice(msg.as_bytes());
        let decoded = decode_response(&wire).expect("should decode bare Error");
        assert_eq!(
            decoded,
            Response::Error {
                message: "something broke".into()
            }
        );
    }

    // -- Frame helpers ------------------------------------------------------

    #[test]
    fn frame_encode_and_read_length() {
        let payload = b"hello";
        let frame = encode_frame(payload);
        assert_eq!(frame.len(), 4 + 5);
        let len = read_frame_length(frame[..4].try_into().unwrap());
        assert_eq!(len, 5);
        assert_eq!(&frame[4..], b"hello");
    }

    #[test]
    fn frame_empty_payload() {
        let frame = encode_frame(&[]);
        assert_eq!(frame.len(), 4);
        let len = read_frame_length(frame[..4].try_into().unwrap());
        assert_eq!(len, 0);
    }

    // -- OpCode conversion --------------------------------------------------

    #[test]
    fn opcode_round_trip() {
        let codes: Vec<u8> = vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x99, 0xA0,
        ];
        for &code in &codes {
            let op = OpCode::from_u8(code).unwrap();
            assert_eq!(op as u8, code);
        }
    }

    // -- Wire format verification -------------------------------------------

    #[test]
    fn heartbeat_wire_format() {
        let wire = encode_request(&Request::Heartbeat);
        // 4-byte header with length=1, then opcode 0x01
        assert_eq!(wire, vec![1, 0, 0, 0, 0x01]);
    }

    #[test]
    fn tap_coord_wire_format() {
        let wire = encode_request(&Request::TapCoord { x: 1, y: 2 });
        // length header: 1 (opcode) + 4 (x) + 4 (y) = 9
        assert_eq!(&wire[..4], &9u32.to_le_bytes());
        assert_eq!(wire[4], OpCode::TapCoord as u8);
        assert_eq!(&wire[5..9], &1i32.to_le_bytes());
        assert_eq!(&wire[9..13], &2i32.to_le_bytes());
    }

    #[test]
    fn response_ok_wire_format() {
        let wire = encode_response(&Response::Ok);
        // length: 1 (opcode 0xA0) + 1 (type 0x00) = 2
        assert_eq!(&wire[..4], &2u32.to_le_bytes());
        assert_eq!(wire[4], OpCode::Response as u8);
        assert_eq!(wire[5], ResponseType::Ok as u8);
    }
}
