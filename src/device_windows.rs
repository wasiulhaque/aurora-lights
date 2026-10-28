//! Windows backend: the controller's HID interface through SetupAPI and hid.dll.
//!
//! `WriteFile` on a HID handle sends an output report on the interrupt OUT
//! endpoint; `HidD_GetInputReport` issues GET_REPORT(Input) for the reply.

use std::mem::{size_of, zeroed};
use std::ptr::{addr_of, null, null_mut};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetHidGuid, HidD_GetInputReport,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile,
};

use crate::controller::{PRODUCT_ID, RawReport, VENDOR_ID};

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful CreateFileW and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// NUL-terminated device interface paths of all present HID devices.
fn hid_device_paths() -> Vec<Vec<u16>> {
    let mut paths = Vec::new();
    // SAFETY: plain SetupAPI enumeration; every buffer passed is sized as the API requires.
    unsafe {
        let mut guid = zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(
            &guid,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if set == INVALID_HANDLE_VALUE as isize {
            return paths;
        }

        for index in 0.. {
            let mut interface: SP_DEVICE_INTERFACE_DATA = zeroed();
            interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, null(), &guid, index, &mut interface) == 0 {
                break;
            }

            let mut required = 0u32;
            SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                null_mut(),
                0,
                &mut required,
                null_mut(),
            );
            if (required as usize) < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() {
                continue;
            }
            // u32 storage keeps the detail struct suitably aligned.
            let mut buffer = vec![0u32; (required as usize).div_ceil(4)];
            let detail = buffer
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                detail,
                required,
                null_mut(),
                null_mut(),
            ) == 0
            {
                continue;
            }

            let start = addr_of!((*detail).DevicePath).cast::<u16>();
            let max_len = (required as usize - (start as usize - detail as usize)) / 2;
            let path = std::slice::from_raw_parts(start, max_len);
            let len = path.iter().position(|&c| c == 0).unwrap_or(max_len);
            let mut path = path[..len].to_vec();
            path.push(0);
            paths.push(path);
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    paths
}

fn open_path(path: &[u16], access: u32) -> Option<Handle> {
    // SAFETY: `path` is NUL-terminated.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    (handle != INVALID_HANDLE_VALUE).then_some(Handle(handle))
}

fn is_controller(path: &[u16]) -> bool {
    // Access 0 is enough to query attributes, even for devices the OS holds exclusively.
    let Some(handle) = open_path(path, 0) else {
        return false;
    };
    // SAFETY: `attributes` is a properly sized out-parameter.
    unsafe {
        let mut attributes: HIDD_ATTRIBUTES = zeroed();
        attributes.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        HidD_GetAttributes(handle.0, &mut attributes)
            && attributes.VendorID == VENDOR_ID
            && attributes.ProductID == PRODUCT_ID
    }
}

fn find_path() -> Option<Vec<u16>> {
    hid_device_paths()
        .into_iter()
        .find(|path| is_controller(path))
}

/// Where the controller is, for display.
pub fn find() -> Option<String> {
    find_path().map(|path| String::from_utf16_lossy(&path[..path.len() - 1]))
}

pub struct Device {
    handle: Handle,
}

impl Device {
    pub fn open() -> Result<Self, String> {
        let path = find_path().ok_or("Alienware AW-ELC controller (187c:0550) not found")?;
        let handle = open_path(&path, GENERIC_READ | GENERIC_WRITE).ok_or_else(|| {
            format!(
                "cannot open the lighting controller: {}",
                std::io::Error::last_os_error()
            )
        })?;
        Ok(Device { handle })
    }

    pub fn write_report(&mut self, report: &RawReport) -> Result<(), String> {
        let mut written = 0u32;
        // SAFETY: `report` is valid for its full length; the handle is synchronous.
        let ok = unsafe {
            WriteFile(
                self.handle.0,
                report.as_ptr(),
                report.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "send to controller: {}",
                std::io::Error::last_os_error()
            ));
        }
        if written as usize != report.len() {
            return Err(format!(
                "short controller write: {written} of {} bytes",
                report.len()
            ));
        }
        Ok(())
    }

    pub fn read_input_report(&mut self, report: &mut RawReport) -> Result<(), String> {
        report[0] = 0;
        // SAFETY: `report` is valid for its full length.
        let ok = unsafe {
            HidD_GetInputReport(
                self.handle.0,
                report.as_mut_ptr().cast(),
                report.len() as u32,
            )
        };
        if !ok {
            return Err(format!(
                "read controller reply: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}
