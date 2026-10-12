use std::io::{self, Write};

use super::{Args, ServeArgs, USAGE, run, write_usage};

#[derive(Debug)]
struct Output {
    bytes: Vec<u8>,
    fail_write: bool,
    fail_flush: bool,
    flushed: bool,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "usage write refused",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushed = true;
        if self.fail_flush {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "usage flush refused",
            ));
        }
        Ok(())
    }
}

#[test]
fn usage_is_written_and_flushed_or_returns_the_write_error() {
    for (fail_write, fail_flush) in [(false, false), (true, false), (false, true)] {
        let mut output = Output {
            bytes: Vec::new(),
            fail_write,
            fail_flush,
            flushed: false,
        };
        let result = write_usage(&mut output);
        if fail_write || fail_flush {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        } else {
            result.unwrap();
            assert_eq!(output.bytes, USAGE.as_bytes());
        }
        assert_eq!(output.flushed, !fail_write);
    }
}

#[test]
fn library_run_refuses_an_invalid_rate_before_opening_the_daemon() {
    for rate in [0, 7_999, 384_001, u32::MAX] {
        let error = run(Args::Serve(ServeArgs {
            rate: Some(rate),
            ..ServeArgs::default()
        }))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "--rate takes a sample rate from 8000 to 384000"
        );
    }
}
