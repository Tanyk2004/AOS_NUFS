use libc::{c_char, c_int, close as c_close, mode_t, open as c_open, O_CREAT, O_RDONLY, O_TRUNC};
use std::ffi::CString;
use std::env;

fn main() {
    // Pick path from first CLI arg or default to a likely non-existent file for demo
    let path = env::args().nth(1).unwrap_or_else(|| String::from("/tmp/demo_open_test.txt"));

    // Convert to C-compatible string (nul-terminated)
    let c_path = match CString::new(path.clone()) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Path contains interior NUL byte: {}", path);
            std::process::exit(1);
        }
    };

    // Flags: try read-only first. You can change flags as needed (e.g., O_RDWR|O_CREAT|O_TRUNC)
    let flags: c_int = O_RDONLY | O_CREAT | O_TRUNC;
    // If using O_CREAT, set mode appropriately, e.g., 0o644
    let mode: mode_t = 0o644;

    // Safety: calling a libc function
    let fd: c_int = unsafe { c_open(c_path.as_ptr() as *const c_char, flags, mode) };

    if fd < 0 {
        // Open failed; print the OS error
        let err = std::io::Error::last_os_error();
        eprintln!("open() failed for {}: {}", path, err);
        std::process::exit(1);
    } else {
        println!("open() succeeded for {}, fd = {}", path, fd);

        // Always close the fd when done
        let rc = unsafe { c_close(fd) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!("close() failed: {}", err);
            std::process::exit(1);
        } else {
            println!("close() succeeded");
        }
    }
}
