//! The terminal calls `lib/io/console.rb` is Ruby over (#110): `tcgetattr(3)`
//! and `tcsetattr(3)`, the window size `ioctl(2)`s, `tcflush(3)` and
//! `ttyname_r(3)`, as io-console 0.8.2's `console.c` makes them on a termios
//! platform.
//!
//! A terminal's mode crosses to Ruby as its flag words, its control
//! characters and its speeds; `lib/io/console.rb` changes them the way
//! `console.c`'s setters do and hands them back. A failed call answers its
//! `io::Error`, which the Ruby side raises.

use std::io;

/// A `struct termios`, as far as Ruby sees one: the four flag words, the
/// control characters, and the input and output speeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    pub iflag: u64,
    pub oflag: u64,
    pub cflag: u64,
    pub lflag: u64,
    pub cc: Vec<u8>,
    pub ispeed: u64,
    pub ospeed: u64,
}

/// `termios` as a `Mode`.
fn mode_from(termios: &libc::termios) -> Mode {
    // SAFETY: the speed getters only read the struct they are given.
    let (ispeed, ospeed) = unsafe { (libc::cfgetispeed(termios), libc::cfgetospeed(termios)) };
    Mode {
        iflag: u64::from(termios.c_iflag),
        oflag: u64::from(termios.c_oflag),
        cflag: u64::from(termios.c_cflag),
        lflag: u64::from(termios.c_lflag),
        cc: termios.c_cc.to_vec(),
        ispeed: u64::from(ispeed),
        ospeed: u64::from(ospeed),
    }
}

/// `mode` written over `termios`, which keeps whatever `Mode` does not carry
/// (Linux's line discipline). A flag word or speed too wide for its field is
/// refused, and control characters past the platform's count are ignored.
fn mode_into(mode: &Mode, termios: &mut libc::termios) -> io::Result<()> {
    let invalid = || io::Error::from_raw_os_error(libc::EINVAL);
    termios.c_iflag = mode.iflag.try_into().map_err(|_| invalid())?;
    termios.c_oflag = mode.oflag.try_into().map_err(|_| invalid())?;
    termios.c_cflag = mode.cflag.try_into().map_err(|_| invalid())?;
    termios.c_lflag = mode.lflag.try_into().map_err(|_| invalid())?;
    for (slot, byte) in termios.c_cc.iter_mut().zip(&mode.cc) {
        *slot = *byte;
    }
    let ispeed: libc::speed_t = mode.ispeed.try_into().map_err(|_| invalid())?;
    let ospeed: libc::speed_t = mode.ospeed.try_into().map_err(|_| invalid())?;
    // SAFETY: the setters only write the struct they are given.
    let failed = unsafe {
        libc::cfsetispeed(termios, ispeed) != 0 || libc::cfsetospeed(termios, ospeed) != 0
    };
    if failed {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `tcgetattr(fd)`.
fn termios_of(fd: libc::c_int) -> io::Result<libc::termios> {
    // SAFETY: all zeroes is a valid `termios`, and `tcgetattr` writes only
    // into the one it is given.
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(termios)
}

/// The terminal mode of `fd`, `console.c`'s `getattr`.
pub fn getattr(fd: libc::c_int) -> io::Result<Mode> {
    termios_of(fd).map(|termios| mode_from(&termios))
}

/// `mode` made the terminal mode of `fd` at once, `console.c`'s `setattr`:
/// `tcsetattr(TCSANOW)`, again when a signal interrupts it.
pub fn setattr(fd: libc::c_int, mode: &Mode) -> io::Result<()> {
    let mut termios = termios_of(fd)?;
    mode_into(mode, &mut termios)?;
    loop {
        // SAFETY: `tcsetattr` only reads the struct it is given.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// The window size of `fd`: rows, columns, and the width and height in
/// pixels, `ioctl(TIOCGWINSZ)`.
pub fn winsize(fd: libc::c_int) -> io::Result<[u16; 4]> {
    // SAFETY: all zeroes is a valid `winsize`, and the `ioctl` writes only
    // into the one it is given.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok([size.ws_row, size.ws_col, size.ws_xpixel, size.ws_ypixel])
}

/// The window size of `fd` set, `ioctl(TIOCSWINSZ)`.
pub fn set_winsize(fd: libc::c_int, [row, col, xpixel, ypixel]: [u16; 4]) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: row,
        ws_col: col,
        ws_xpixel: xpixel,
        ws_ypixel: ypixel,
    };
    // SAFETY: the `ioctl` only reads the struct it is given.
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `tcflush(fd, queue)`.
pub fn flush(fd: libc::c_int, queue: libc::c_int) -> io::Result<()> {
    // SAFETY: no memory is passed.
    if unsafe { libc::tcflush(fd, queue) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The name of the terminal `fd` is, or `None` when it is not one:
/// `isatty(3)`, then `ttyname_r(3)` into a buffer grown while it is too small.
pub fn ttyname(fd: libc::c_int) -> io::Result<Option<Vec<u8>>> {
    // SAFETY: no memory is passed.
    if unsafe { libc::isatty(fd) } == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0u8; 1024];
    loop {
        // SAFETY: the buffer is as long as the length given.
        let error = unsafe { libc::ttyname_r(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if error == 0 {
            let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
            buffer.truncate(end);
            return Ok(Some(buffer));
        }
        if error != libc::ERANGE || buffer.len() >= i32::MAX as usize / 2 {
            return Err(io::Error::from_raw_os_error(error));
        }
        buffer.resize(buffer.len() * 2, 0);
    }
}

/// The termios flags, control character indices and queue selectors
/// `console.c` uses, by name, and `NCCS`, the count of control characters.
pub fn constants() -> Vec<(&'static str, i64)> {
    // The flags' type, `tcflag_t`, is `u32` on Linux and `u64` on macOS, so
    // the conversion is fallible on some platform; every value fits.
    fn value<T: TryInto<i64>>(value: T) -> i64 {
        value
            .try_into()
            .ok()
            .expect("a termios constant fits in i64")
    }
    macro_rules! table {
        ($($name:ident),* $(,)?) => {
            vec![$((stringify!($name), value(libc::$name))),*]
        };
    }
    let mut names = table![
        IGNBRK, BRKINT, PARMRK, ISTRIP, INLCR, IGNCR, ICRNL, IXON, OPOST, CSIZE, PARENB, CS8, ECHO,
        ECHOE, ECHOK, ECHONL, ICANON, ISIG, IEXTEN, TCIFLUSH, TCOFLUSH, TCIOFLUSH,
    ];
    // Indices into `c_cc`, which the libc crate gives as `usize`.
    for (name, index) in [
        ("VMIN", libc::VMIN),
        ("VTIME", libc::VTIME),
        ("NCCS", libc::NCCS),
    ] {
        names.push((name, value(index)));
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    // The conversions between `termios` and `Mode`: run under miri against a
    // struct built here, since miri cannot call the terminal's functions.

    fn sample() -> libc::termios {
        // SAFETY: all zeroes is a valid `termios`.
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        termios.c_iflag = libc::ICRNL | libc::IXON;
        termios.c_oflag = libc::OPOST;
        termios.c_cflag = libc::CS8;
        termios.c_lflag = libc::ECHO | libc::ICANON;
        termios.c_cc[libc::VMIN] = 1;
        termios.c_cc[libc::VTIME] = 3;
        termios
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_mode_round_trips_through_termios() {
        let termios = sample();
        let mode = mode_from(&termios);
        assert_eq!(mode.lflag, u64::from(libc::ECHO | libc::ICANON));
        assert_eq!(mode.cc.len(), termios.c_cc.len());
        assert_eq!(mode.cc[libc::VTIME], 3);
        // SAFETY: all zeroes is a valid `termios`.
        let mut back: libc::termios = unsafe { std::mem::zeroed() };
        mode_into(&mode, &mut back).expect("the mode fits");
        assert_eq!(mode_from(&back), mode);
    }

    #[test]
    fn mode_into_refuses_a_flag_word_too_wide_and_ignores_extra_characters() {
        let mut termios = sample();
        let mut mode = Mode {
            iflag: 0,
            oflag: 0,
            cflag: 0,
            lflag: u64::MAX,
            cc: vec![9; 200],
            ispeed: 0,
            ospeed: 0,
        };
        // A flag word is only too wide where `tcflag_t` is narrower than
        // 64 bits: Linux's is `u32`, macOS's is `u64`.
        if std::mem::size_of::<libc::tcflag_t>() < std::mem::size_of::<u64>() {
            let error = mode_into(&mode, &mut termios).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
        }
        mode.lflag = u64::from(libc::ECHO);
        // The speeds go through the C library, which miri cannot call.
        if cfg!(not(miri)) {
            mode_into(&mode, &mut termios).expect("the mode fits");
            assert!(termios.c_cc.iter().all(|&c| c == 9));
            assert_eq!(termios.c_lflag, libc::ECHO);
        }
    }

    // The calls themselves, which miri cannot make.

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_descriptor_that_is_no_terminal_is_refused() {
        let mut fds = [0; 2];
        // SAFETY: `pipe` writes two descriptors into the array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let enotty = Some(libc::ENOTTY);
        assert_eq!(getattr(fds[0]).unwrap_err().raw_os_error(), enotty);
        assert_eq!(winsize(fds[1]).unwrap_err().raw_os_error(), enotty);
        assert_eq!(
            set_winsize(fds[1], [24, 80, 0, 0])
                .unwrap_err()
                .raw_os_error(),
            enotty
        );
        assert_eq!(
            flush(fds[0], libc::TCIFLUSH).unwrap_err().raw_os_error(),
            enotty
        );
        assert_eq!(ttyname(fds[0]).unwrap(), None);
        assert_eq!(getattr(-1).unwrap_err().raw_os_error(), Some(libc::EBADF));
        // SAFETY: the two descriptors `pipe` made.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_pseudo_terminal_keeps_its_mode_and_size() {
        // SAFETY: `posix_openpt` and the calls after it take no memory but
        // the descriptor, and `ptsname` returns a NUL-terminated name that
        // is copied out before any other call can overwrite it.
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            if master < 0 {
                return; // No pseudo-terminals here.
            }
            assert_eq!(libc::grantpt(master), 0);
            assert_eq!(libc::unlockpt(master), 0);
            // `ptsname_r` is not in macOS's libc; this test is the only
            // caller of `ptsname`, so its static buffer is not shared.
            let name = libc::ptsname(master);
            assert!(!name.is_null());
            let name = std::ffi::CStr::from_ptr(name).to_owned();
            let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(slave >= 0);

            let mut mode = getattr(slave).expect("a terminal has a mode");
            mode.lflag &= !u64::from(libc::ECHO);
            mode.cc[libc::VMIN] = 0;
            setattr(slave, &mode).expect("the mode is set");
            assert_eq!(getattr(slave).unwrap(), mode);

            set_winsize(slave, [40, 100, 7, 9]).expect("the size is set");
            assert_eq!(winsize(slave).unwrap(), [40, 100, 7, 9]);
            flush(slave, libc::TCIOFLUSH).expect("the queues are flushed");
            let tty = ttyname(slave).unwrap().expect("a terminal has a name");
            assert_eq!(tty, name.to_bytes());

            libc::close(slave);
            libc::close(master);
        }
    }
}
