use std::{ffi::c_void, mem};

use windows_sys::Win32::{
    Devices::HumanInterfaceDevice::{
        HidP_GetCaps, HidP_GetUsageValue, HidP_GetValueCaps, HidP_Input, HIDP_CAPS,
        HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS,
    },
    UI::Input::{
        GetRawInputData, GetRawInputDeviceInfoA, HRAWINPUT, RAWINPUT, RAWINPUTHEADER,
        RIDI_PREPARSEDDATA, RID_INPUT,
    },
};

use crate::advanced::AxisRanges;
use crate::gesture::Contact;

pub(super) struct ParsedContactReport {
    pub device_handle: isize,
    pub contacts: Vec<Contact>,
    pub reported_count: u32,
    pub ranges: AxisRanges,
}

#[derive(Default)]
struct ContactFields {
    id: Option<i32>,
    x: Option<i32>,
    y: Option<i32>,
}

impl ContactFields {
    fn take_complete(&mut self) -> Option<Contact> {
        let contact = Contact {
            id: self.id?,
            x: self.x?,
            y: self.y?,
        };
        *self = Self::default();
        Some(contact)
    }
}

pub(super) unsafe fn parse_contact_report(
    raw_input_handle: HRAWINPUT,
) -> Option<ParsedContactReport> {
    let header_size = mem::size_of::<RAWINPUTHEADER>() as u32;
    let mut raw_size = 0_u32;
    if GetRawInputData(
        raw_input_handle,
        RID_INPUT,
        std::ptr::null_mut(),
        &mut raw_size,
        header_size,
    ) != 0
    {
        return None;
    }

    let mut raw_buffer = vec![0_u8; raw_size as usize];
    if GetRawInputData(
        raw_input_handle,
        RID_INPUT,
        raw_buffer.as_mut_ptr().cast::<c_void>(),
        &mut raw_size,
        header_size,
    ) != raw_size
    {
        return None;
    }
    if raw_buffer.len() < mem::size_of::<RAWINPUT>() {
        return None;
    }

    let raw = std::ptr::read_unaligned(raw_buffer.as_ptr().cast::<RAWINPUT>());
    let hid = raw.data.hid;
    let hid_length = hid.dwSizeHid.checked_mul(hid.dwCount)? as usize;
    let hid_offset = (raw_size as usize).checked_sub(hid_length)?;
    let hid_bytes = raw_buffer.get(hid_offset..hid_offset + hid_length)?;

    let mut preparsed_size = 0_u32;
    let handle = raw.header.hDevice;
    if GetRawInputDeviceInfoA(
        handle,
        RIDI_PREPARSEDDATA,
        std::ptr::null_mut(),
        &mut preparsed_size,
    ) != 0
    {
        return None;
    }
    if preparsed_size == 0 {
        return None;
    }

    let mut preparsed = vec![0_u8; preparsed_size as usize];
    if GetRawInputDeviceInfoA(
        handle,
        RIDI_PREPARSEDDATA,
        preparsed.as_mut_ptr().cast::<c_void>(),
        &mut preparsed_size,
    ) != preparsed_size
    {
        return None;
    }
    let preparsed_handle = preparsed.as_mut_ptr() as isize;

    let mut capabilities = HIDP_CAPS::default();
    if HidP_GetCaps(preparsed_handle, &mut capabilities) != HIDP_STATUS_SUCCESS {
        return None;
    }

    let mut value_count = capabilities.NumberInputValueCaps;
    let mut value_caps = vec![HIDP_VALUE_CAPS::default(); value_count as usize];
    if HidP_GetValueCaps(
        HidP_Input,
        value_caps.as_mut_ptr(),
        &mut value_count,
        preparsed_handle,
    ) != HIDP_STATUS_SUCCESS
    {
        return None;
    }
    value_caps.truncate(value_count as usize);
    value_caps.sort_by_key(|capability| capability.LinkCollection);

    let mut reported_count = 99_u32;
    let mut ranges = AxisRanges::default();
    let mut creators: Vec<ContactFields> = Vec::new();
    let mut contacts = Vec::new();

    'capabilities: for capability in value_caps {
        let usage = capability.Anonymous.NotRange.Usage;
        if capability.LinkCollection != 0 && capability.UsagePage == 0x01 {
            match usage {
                0x30 => {
                    ranges.min_x = capability.LogicalMin;
                    ranges.max_x = capability.LogicalMax;
                }
                0x31 => {
                    ranges.min_y = capability.LogicalMin;
                    ranges.max_y = capability.LogicalMax;
                }
                _ => {}
            }
        }
        for report_index in 0..hid.dwCount as usize {
            while creators.len() <= report_index {
                creators.push(ContactFields::default());
            }

            let report_offset = report_index.checked_mul(hid.dwSizeHid as usize)?;
            let report = hid_bytes.get(report_offset..report_offset + hid.dwSizeHid as usize)?;
            let mut value = 0_u32;
            if HidP_GetUsageValue(
                HidP_Input,
                capability.UsagePage,
                capability.LinkCollection,
                usage,
                &mut value,
                preparsed_handle,
                report.as_ptr(),
                report.len() as u32,
            ) != HIDP_STATUS_SUCCESS
            {
                continue;
            }

            if capability.LinkCollection == 0 {
                if capability.UsagePage == 0x0D && usage == 0x54 {
                    reported_count = value;
                }
            } else {
                let creator = &mut creators[report_index];
                match (capability.UsagePage, usage) {
                    (0x0D, 0x51) => creator.id = Some(value as i32),
                    (0x01, 0x30) => creator.x = Some(value as i32),
                    (0x01, 0x31) => creator.y = Some(value as i32),
                    _ => {}
                }
            }
        }

        for creator in &mut creators {
            if (reported_count == 0 || contacts.len() < reported_count as usize)
                && creator.id.is_some()
                && creator.x.is_some()
                && creator.y.is_some()
            {
                if let Some(contact) = creator.take_complete() {
                    contacts.push(contact);
                }
            }
        }
        if reported_count != 0 && contacts.len() >= reported_count as usize {
            break 'capabilities;
        }
    }

    Some(ParsedContactReport {
        device_handle: handle as isize,
        contacts,
        reported_count,
        ranges,
    })
}
