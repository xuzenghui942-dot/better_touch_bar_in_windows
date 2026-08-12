use std::{collections::HashMap, ffi::c_void, mem};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::HANDLE,
    UI::Input::{
        GetRawInputDeviceInfoA, GetRawInputDeviceList, RAWINPUTDEVICELIST, RIDI_DEVICEINFO,
        RIDI_DEVICENAME, RID_DEVICE_INFO, RIM_TYPEHID,
    },
};

use super::stable_device_id;

const ERROR_SENTINEL: u32 = u32::MAX;
const DIGITIZER_USAGE_PAGE: u16 = 0x000D;
const TOUCHPAD_USAGE: u16 = 0x0005;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub id: String,
    pub vendor_id: String,
    pub product_id: String,
}

impl DeviceDescriptor {
    pub fn label(&self) -> String {
        format!("{}({}:{})", self.id, self.product_id, self.vendor_id)
    }
}

#[derive(Default)]
pub(super) struct DeviceCatalog {
    devices: HashMap<isize, DeviceDescriptor>,
}

impl DeviceCatalog {
    pub fn enumerate(&mut self) -> bool {
        let mut count = 0_u32;
        let entry_size = mem::size_of::<RAWINPUTDEVICELIST>() as u32;
        let first = unsafe { GetRawInputDeviceList(std::ptr::null_mut(), &mut count, entry_size) };
        if first != 0 {
            return false;
        }

        let mut entries = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let received =
            unsafe { GetRawInputDeviceList(entries.as_mut_ptr(), &mut count, entry_size) };
        if received != count {
            return false;
        }

        let mut found = false;
        for entry in entries {
            if entry.dwType == RIM_TYPEHID {
                found |= self.inspect(entry.hDevice).is_some();
            }
        }
        found
    }

    pub fn inspect(&mut self, handle: HANDLE) -> Option<DeviceDescriptor> {
        let key = handle as isize;
        if let Some(existing) = self.devices.get(&key) {
            return Some(existing.clone());
        }

        let descriptor = query_touchpad(handle)?;
        self.devices.insert(key, descriptor.clone());
        Some(descriptor)
    }

    pub fn refresh(&mut self, handle: HANDLE) -> bool {
        let key = handle as isize;
        self.devices.remove(&key);
        self.inspect(handle).is_some()
    }

    pub fn remove(&mut self, handle: HANDLE) {
        self.devices.remove(&(handle as isize));
    }

    pub fn all(&self) -> Vec<DeviceDescriptor> {
        let mut devices: Vec<_> = self.devices.values().cloned().collect();
        devices.sort_by(|left, right| left.id.cmp(&right.id));
        devices
    }
}

fn query_touchpad(handle: HANDLE) -> Option<DeviceDescriptor> {
    let mut information = RID_DEVICE_INFO {
        cbSize: mem::size_of::<RID_DEVICE_INFO>() as u32,
        ..RID_DEVICE_INFO::default()
    };
    let mut size = information.cbSize;
    let result = unsafe {
        GetRawInputDeviceInfoA(
            handle,
            RIDI_DEVICEINFO,
            (&mut information as *mut RID_DEVICE_INFO).cast::<c_void>(),
            &mut size,
        )
    };
    if result == ERROR_SENTINEL {
        return None;
    }

    let hid = unsafe { information.Anonymous.hid };
    if hid.usUsagePage != DIGITIZER_USAGE_PAGE || hid.usUsage != TOUCHPAD_USAGE {
        return None;
    }

    let name = query_device_name(handle).unwrap_or_default();
    Some(DeviceDescriptor {
        id: stable_device_id(&name),
        vendor_id: hid.dwVendorId.to_string(),
        product_id: hid.dwProductId.to_string(),
    })
}

fn query_device_name(handle: HANDLE) -> Option<String> {
    let mut length = 0_u32;
    let first = unsafe {
        GetRawInputDeviceInfoA(handle, RIDI_DEVICENAME, std::ptr::null_mut(), &mut length)
    };
    if first == ERROR_SENTINEL || length == 0 {
        return None;
    }

    let mut bytes = vec![0_u8; length as usize + 1];
    let second = unsafe {
        GetRawInputDeviceInfoA(
            handle,
            RIDI_DEVICENAME,
            bytes.as_mut_ptr().cast::<c_void>(),
            &mut length,
        )
    };
    if second == ERROR_SENTINEL {
        return None;
    }

    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}
