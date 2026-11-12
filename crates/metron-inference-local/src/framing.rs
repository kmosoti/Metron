//! Length-delimited messages, rejecting oversized lengths before allocating.
use std::io::{self, Read, Write};

pub const MAX_FRAME: usize = 64 * 1024;

pub fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    read_bounded_frame(input, MAX_FRAME)
}

pub fn read_request_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    read_bounded_frame(input, metron_protocol::MAX_LOCAL_REQUEST_FRAME)
}

fn read_bounded_frame(input: &mut impl Read, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0_u8; 4];
    match input.read(&mut header[..1])? {
        0 => return Ok(None),
        1 => (),
        _ => unreachable!(),
    }
    input.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut frame = vec![0; length];
    input.read_exact(&mut frame)?;
    Ok(Some(frame))
}

pub fn write_frame(output: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    write_bounded_frame(output, frame, MAX_FRAME)
}

pub fn write_request_frame(output: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    write_bounded_frame(output, frame, metron_protocol::MAX_LOCAL_REQUEST_FRAME)
}

fn write_bounded_frame(output: &mut impl Write, frame: &[u8], limit: usize) -> io::Result<()> {
    if frame.is_empty() || frame.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid frame length",
        ));
    }
    output.write_all(&(frame.len() as u32).to_be_bytes())?;
    output.write_all(frame)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_boundaries_and_truncated_header() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &vec![7; MAX_FRAME]).unwrap();
        assert_eq!(
            read_frame(&mut &wire[..]).unwrap().unwrap().len(),
            MAX_FRAME
        );
        assert!(read_frame(&mut &u32::MAX.to_be_bytes()[..]).is_err());
        assert!(read_frame(&mut &wire[..2]).is_err());
        assert!(write_frame(&mut Vec::new(), &vec![0; MAX_FRAME + 1]).is_err());
    }

    #[test]
    fn larger_requests_do_not_widen_reply_frames() {
        let body = vec![7; metron_protocol::MAX_LOCAL_REQUEST_FRAME];
        let mut wire = Vec::new();
        write_request_frame(&mut wire, &body).unwrap();
        assert_eq!(read_request_frame(&mut &wire[..]).unwrap(), Some(body));
        assert!(read_frame(&mut &wire[..]).is_err());
        assert!(write_frame(&mut Vec::new(), &wire[4..]).is_err());
        assert!(read_request_frame(&mut &u32::MAX.to_be_bytes()[..]).is_err());
        assert!(
            write_request_frame(
                &mut Vec::new(),
                &vec![0; metron_protocol::MAX_LOCAL_REQUEST_FRAME + 1]
            )
            .is_err()
        );
    }
}
