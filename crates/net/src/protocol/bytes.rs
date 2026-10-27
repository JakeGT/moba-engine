//! Little-endian byte writers, the bounds-checked [`Reader`] and clamping helpers.

use super::WireError;

pub(super) fn clamp_u16(value: i32) -> u16 {
    value.clamp(0, i32::from(u16::MAX)) as u16
}

pub(super) fn clamp_u16_u32(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub(super) fn count_u16(count: usize) -> Result<u16, WireError> {
    u16::try_from(count).map_err(|_| WireError::TooMany)
}

pub(super) fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(super) struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub(super) const fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub(super) const fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    pub(super) fn u8(&mut self) -> Result<u8, WireError> {
        let byte = *self.data.get(self.position).ok_or(WireError::Truncated)?;
        self.position += 1;
        Ok(byte)
    }

    pub(super) fn u16(&mut self) -> Result<u16, WireError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(super) fn u32(&mut self) -> Result<u32, WireError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(super) fn u64(&mut self) -> Result<u64, WireError> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub(super) fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_le_bytes(
            self.bytes(4)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ))
    }

    pub(super) fn i64(&mut self) -> Result<i64, WireError> {
        Ok(i64::from_le_bytes(
            self.bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ))
    }

    pub(super) fn bytes(&mut self, length: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(WireError::Truncated)?;
        let slice = self
            .data
            .get(self.position..end)
            .ok_or(WireError::Truncated)?;
        self.position = end;
        Ok(slice)
    }
}
