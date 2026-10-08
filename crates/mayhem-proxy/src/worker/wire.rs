use super::{Error, Result};
use std::io::{Read, Write};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const HELLO: u8 = 1;
pub(super) const CHUNK: u8 = 2;
pub(super) const FINISH: u8 = 3;
pub(super) const POLICY_CHUNK: u8 = 4;
pub(super) const POLICY_END: u8 = 5;
pub(super) const FRAME_END: u8 = 6;
pub(super) const VERIFY_CHUNK: u8 = 7;
pub(super) const VERIFY_END: u8 = 8;
pub(super) const READY: u8 = 16;
pub(super) const EVENT: u8 = 17;
pub(super) const ACK: u8 = 18;
pub(super) const END: u8 = 19;
pub(super) const FAILURE: u8 = 20;

pub(super) struct Packet {
    pub kind: u8,
    pub bytes: Vec<u8>,
}
fn header(kind: u8, len: usize) -> Result<[u8; 5]> {
    let n = u32::try_from(len).map_err(|_| Error::Protocol)?;
    let mut bytes = [0; 5];
    bytes[0] = kind;
    bytes[1..].copy_from_slice(&n.to_le_bytes());
    Ok(bytes)
}
fn size(header: &[u8; 5], max: usize) -> Result<usize> {
    let n = u32::from_le_bytes(header[1..].try_into().map_err(|_| Error::Protocol)?) as usize;
    if n > max {
        Err(Error::Protocol)
    } else {
        Ok(n)
    }
}
pub(super) fn write_packet(w: &mut impl Write, kind: u8, bytes: &[u8]) -> Result<()> {
    w.write_all(&header(kind, bytes.len())?)
        .map_err(|_| Error::Stopped)?;
    w.write_all(bytes).map_err(|_| Error::Stopped)?;
    w.flush().map_err(|_| Error::Stopped)
}
pub(super) fn read_packet(r: &mut impl Read, max: usize) -> Result<Option<Packet>> {
    let mut header = [0; 5];
    if r.read(&mut header[..1]).map_err(|_| Error::Stopped)? == 0 {
        return Ok(None);
    }
    r.read_exact(&mut header[1..])
        .map_err(|_| Error::Protocol)?;
    let mut bytes = vec![0; size(&header, max)?];
    r.read_exact(&mut bytes).map_err(|_| Error::Protocol)?;
    Ok(Some(Packet {
        kind: header[0],
        bytes,
    }))
}
pub(super) async fn send(w: &mut (impl AsyncWrite + Unpin), kind: u8, bytes: &[u8]) -> Result<()> {
    w.write_all(&header(kind, bytes.len())?)
        .await
        .map_err(|_| Error::Stopped)?;
    w.write_all(bytes).await.map_err(|_| Error::Stopped)?;
    w.flush().await.map_err(|_| Error::Stopped)
}
pub(super) async fn receive(r: &mut (impl AsyncRead + Unpin), max: usize) -> Result<Packet> {
    let mut header = [0; 5];
    r.read_exact(&mut header)
        .await
        .map_err(|_| Error::Stopped)?;
    let max = match header[0] {
        READY | FAILURE => max.min(super::CONTROL_BYTES),
        ACK | END => max.min(8),
        EVENT => max,
        _ => return Err(Error::Protocol),
    };
    let mut bytes = vec![0; size(&header, max)?];
    r.read_exact(&mut bytes)
        .await
        .map_err(|_| Error::Protocol)?;
    Ok(Packet {
        kind: header[0],
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sync_lengths_are_checked_before_body_allocation_and_truncation_is_not_eof() {
        assert!(read_packet(&mut &[][..], 20).unwrap().is_none());
        for bytes in [
            vec![HELLO],
            vec![HELLO, 255, 255, 255, 255],
            vec![HELLO, 2, 0, 0, 0, b'{'],
        ] {
            assert!(matches!(
                read_packet(&mut bytes.as_slice(), 20),
                Err(Error::Protocol)
            ));
        }
    }
    #[tokio::test]
    async fn control_packet_byte_limits_apply_before_body_read() {
        for (kind, length) in [
            (READY, 5000),
            (FAILURE, 5000),
            (ACK, 9),
            (END, 9),
            (99, 1),
            (EVENT, 1_000_001),
        ] {
            let (mut writer, mut reader) = tokio::io::duplex(8);
            writer
                .write_all(&header(kind, length).unwrap())
                .await
                .unwrap();
            // Writer stays open; a decoder that waits for the claimed body hangs.
            assert!(matches!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    receive(&mut reader, 1_000_000)
                )
                .await
                .unwrap(),
                Err(Error::Protocol)
            ));
        }
    }
}
