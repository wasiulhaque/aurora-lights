//! Linux backend: the controller's hidraw node.
//!
//! Writes on hidraw go out on the interrupt OUT endpoint; HIDIOCGINPUT issues
//! GET_REPORT(Input) for the reply.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use crate::controller::{PRODUCT_ID, RawReport, VENDOR_ID};

// _IOC(_IOC_READ | _IOC_WRITE, 'H', 0x0A, len)
const HIDIOCGINPUT: libc::c_ulong = (3 << 30)
    | ((size_of::<RawReport>() as libc::c_ulong) << 16)
    | ((b'H' as libc::c_ulong) << 8)
    | 0x0A;

/// Path of the controller's hidraw node, found through sysfs.
fn find_hidraw() -> Option<PathBuf> {
    let hid_id = format!("HID_ID=0003:0000{VENDOR_ID:04X}:0000{PRODUCT_ID:04X}");
    let mut nodes: Vec<_> = fs::read_dir("/sys/class/hidraw").ok()?.flatten().collect();
    nodes.sort_by_key(fs::DirEntry::file_name);
    nodes.into_iter().find_map(|entry| {
        let uevent = fs::read_to_string(entry.path().join("device/uevent")).ok()?;
        uevent
            .lines()
            .any(|line| line == hid_id)
            .then(|| PathBuf::from("/dev").join(entry.file_name()))
    })
}

/// Where the controller is, for display.
pub fn find() -> Option<String> {
    find_hidraw().map(|path| path.display().to_string())
}

pub struct Device {
    file: File,
}

impl Device {
    pub fn open() -> Result<Self, String> {
        let path = find_hidraw().ok_or("Alienware AW-ELC controller (187c:0550) not found")?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| {
                let hint = if e.kind() == ErrorKind::PermissionDenied {
                    " (install the udev rule from the README, then log out and back in)"
                } else {
                    ""
                };
                format!("cannot open {}: {e}{hint}", path.display())
            })?;
        Ok(Device { file })
    }

    pub fn write_report(&mut self, report: &RawReport) -> Result<(), String> {
        let sent = self
            .file
            .write(report)
            .map_err(|e| format!("send to controller: {e}"))?;
        if sent != report.len() {
            return Err(format!(
                "short controller write: {sent} of {} bytes",
                report.len()
            ));
        }
        Ok(())
    }

    pub fn read_input_report(&mut self, report: &mut RawReport) -> Result<(), String> {
        report[0] = 0;
        // SAFETY: `report` is exactly the length encoded in HIDIOCGINPUT.
        let rc = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                HIDIOCGINPUT as _,
                report.as_mut_ptr(),
            )
        };
        if rc < 0 {
            return Err(format!(
                "read controller reply: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}
