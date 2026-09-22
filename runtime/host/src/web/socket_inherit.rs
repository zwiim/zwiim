use std::io;
use tokio::net::TcpListener;

/// Mark the listening socket so child processes spawned later (terminals,
/// ACP CLIs, git, etc.) cannot inherit the socket handle.
///
/// On Windows `CreateProcess` defaults to `bInheritHandles=TRUE`; without
/// this call any leaked inheritable handle keeps the LISTEN port alive
/// inside a child even after codeg exits — observed as "PID gone but
/// `:3080` still LISTENING" on issue #126.
///
/// On Unix Tokio/Mio already creates sockets with `SOCK_CLOEXEC` /
/// `FD_CLOEXEC`, but we re-set the flag defensively so the invariant is
/// guaranteed at the codeg layer regardless of upstream changes.
pub fn mark_listener_non_inheritable(listener: &TcpListener) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};
        // RawSocket is u64 on Windows but the actual SOCKET value fits in
        // a pointer-width integer (32-bit on x86, 64-bit on x64). Going
        // through `usize` is the canonical idiom that's correct on both.
        let raw = listener.as_raw_socket() as usize as HANDLE;
        let ok = unsafe { SetHandleInformation(raw, HANDLE_FLAG_INHERIT, 0) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = listener.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let new_flags = flags | libc::FD_CLOEXEC;
        if new_flags != flags {
            let ret = unsafe { libc::fcntl(fd, libc::F_SETFD, new_flags) };
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}
