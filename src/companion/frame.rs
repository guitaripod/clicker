//! Companion frame types and the 4-byte transport header.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Unknown,
    NoOp,
    PsStart,
    PsNext,
    PvStart,
    PvNext,
    UOpack,
    EOpack,
    POpack,
    PaReq,
    PaRsp,
    SessionStartRequest,
    SessionStartResponse,
    SessionData,
    FamilyIdentityRequest,
    FamilyIdentityResponse,
    FamilyIdentityUpdate,
}

impl FrameType {
    pub fn code(self) -> u8 {
        match self {
            FrameType::Unknown => 0,
            FrameType::NoOp => 1,
            FrameType::PsStart => 3,
            FrameType::PsNext => 4,
            FrameType::PvStart => 5,
            FrameType::PvNext => 6,
            FrameType::UOpack => 7,
            FrameType::EOpack => 8,
            FrameType::POpack => 9,
            FrameType::PaReq => 10,
            FrameType::PaRsp => 11,
            FrameType::SessionStartRequest => 16,
            FrameType::SessionStartResponse => 17,
            FrameType::SessionData => 18,
            FrameType::FamilyIdentityRequest => 32,
            FrameType::FamilyIdentityResponse => 33,
            FrameType::FamilyIdentityUpdate => 34,
        }
    }

    pub fn from_code(code: u8) -> FrameType {
        match code {
            0 => FrameType::Unknown,
            1 => FrameType::NoOp,
            3 => FrameType::PsStart,
            4 => FrameType::PsNext,
            5 => FrameType::PvStart,
            6 => FrameType::PvNext,
            7 => FrameType::UOpack,
            8 => FrameType::EOpack,
            9 => FrameType::POpack,
            10 => FrameType::PaReq,
            11 => FrameType::PaRsp,
            16 => FrameType::SessionStartRequest,
            17 => FrameType::SessionStartResponse,
            18 => FrameType::SessionData,
            32 => FrameType::FamilyIdentityRequest,
            33 => FrameType::FamilyIdentityResponse,
            34 => FrameType::FamilyIdentityUpdate,
            _ => FrameType::Unknown,
        }
    }

    pub fn is_opack(self) -> bool {
        matches!(
            self,
            FrameType::UOpack | FrameType::EOpack | FrameType::POpack
        )
    }
}

pub const HEADER_LEN: usize = 4;
pub const AUTH_TAG_LEN: usize = 16;

pub fn header(frame_type: FrameType, payload_len: usize) -> [u8; HEADER_LEN] {
    let len = payload_len as u32;
    [
        frame_type.code(),
        (len >> 16) as u8,
        (len >> 8) as u8,
        len as u8,
    ]
}

pub fn payload_len(header: &[u8; HEADER_LEN]) -> usize {
    ((header[1] as usize) << 16) | ((header[2] as usize) << 8) | header[3] as usize
}
