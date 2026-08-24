#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct WindowsGpuEvent {
    pub(super) event_id: u32,
    /// Raw GPUID from event XML (matches `GpuId.0` which = pci_bus * 256).
    /// `None` for system-wide events (e.g. `\Device\Video3`) that carry no GPUID.
    pub(super) gpu_bus_id: Option<u32>,
    /// True when the event XML contains Graphics FECS Exception.
    pub(super) is_fecs: bool,
    /// True when the event XML contains Restarting TDR or Reset TDR.
    pub(super) is_tdr: bool,
}

#[cfg(windows)]
const EVENT_QUERY: &str = "*[System[(EventID=153 or EventID=13 or EventID=14 or EventID=4101 or EventID=10110 or EventID=10111)]]";
#[cfg(windows)]
const EVENT_LOGS: [&str; 2] = [
    "System",
    "Microsoft-Windows-DriverFrameworks-UserMode/Operational",
];

#[cfg(windows)]
pub(super) fn query_windows_gpu_events(
    start: SystemTime,
    end: SystemTime,
) -> Option<Vec<WindowsGpuEvent>> {
    let start_ms = unix_millis(start)?;
    let end_ms = unix_millis(end)?;
    if end_ms < start_ms {
        return Some(Vec::new());
    }

    let mut events = Vec::new();
    for log in EVENT_LOGS {
        query_log_events(log, start_ms, end_ms, &mut events);
    }

    Some(events)
}

#[cfg(windows)]
fn query_log_events(log: &str, start_ms: i128, end_ms: i128, events: &mut Vec<WindowsGpuEvent>) {
    use std::iter::once;
    use windows_sys::Win32::Foundation::{
        ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, GetLastError,
    };
    use windows_sys::Win32::System::EventLog::{
        EVT_HANDLE, EvtClose, EvtNext, EvtQuery, EvtQueryChannelPath, EvtQueryReverseDirection,
        EvtRender, EvtRenderEventXml,
    };

    struct EvtHandle(EVT_HANDLE);

    impl Drop for EvtHandle {
        fn drop(&mut self) {
            if self.0 != 0 {
                // SAFETY: The handle comes from a successful Windows Event Log API call.
                unsafe {
                    EvtClose(self.0);
                }
            }
        }
    }

    let log_wide = wide_null(log);
    let query_wide = EVENT_QUERY
        .encode_utf16()
        .chain(once(0))
        .collect::<Vec<_>>();
    // SAFETY: Pointers are valid NUL-terminated UTF-16 strings for the duration of the call.
    let query_handle = unsafe {
        EvtQuery(
            0,
            log_wide.as_ptr(),
            query_wide.as_ptr(),
            EvtQueryChannelPath | EvtQueryReverseDirection,
        )
    };
    if query_handle == 0 {
        return;
    }
    let query_handle = EvtHandle(query_handle);

    loop {
        let mut handles: [EVT_HANDLE; 16] = [0; 16];
        let mut returned = 0u32;
        // SAFETY: `handles` is a valid output buffer and `returned` is a valid out pointer.
        let ok = unsafe {
            EvtNext(
                query_handle.0,
                handles.len() as u32,
                handles.as_mut_ptr(),
                0,
                0,
                &mut returned,
            )
        };
        if ok == 0 {
            // SAFETY: GetLastError has no preconditions.
            if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                break;
            }
            break;
        }

        let mut reached_older_event = false;
        for handle in handles.iter().take(returned as usize).copied() {
            let event_handle = EvtHandle(handle);
            if reached_older_event {
                continue;
            }
            let Some(xml) = render_event_xml(event_handle.0) else {
                continue;
            };
            let Some((event_ms, event)) = event_from_xml(&xml) else {
                continue;
            };

            if event_ms > end_ms {
                continue;
            }
            if event_ms < start_ms {
                reached_older_event = true;
                continue;
            }
            events.push(event);
        }

        if reached_older_event {
            break;
        }
    }

    fn render_event_xml(handle: EVT_HANDLE) -> Option<String> {
        let mut buffer_used = 0u32;
        let mut property_count = 0u32;
        // SAFETY: The zero-sized probe call follows the documented EvtRender pattern.
        let ok = unsafe {
            EvtRender(
                0,
                handle,
                EvtRenderEventXml,
                0,
                std::ptr::null_mut(),
                &mut buffer_used,
                &mut property_count,
            )
        };
        // SAFETY: GetLastError has no preconditions.
        if ok != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || buffer_used == 0 {
            return None;
        }

        let mut buffer = vec![0u16; buffer_used.div_ceil(2) as usize];
        // SAFETY: `buffer` has at least `buffer_used` bytes and all out pointers are valid.
        let ok = unsafe {
            EvtRender(
                0,
                handle,
                EvtRenderEventXml,
                buffer_used,
                buffer.as_mut_ptr().cast(),
                &mut buffer_used,
                &mut property_count,
            )
        };
        if ok == 0 {
            return None;
        }

        let len = buffer
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn unix_millis(value: SystemTime) -> Option<i128> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as i128)
}

fn event_from_xml(xml: &str) -> Option<(i128, WindowsGpuEvent)> {
    let event_ms = parse_windows_event_time_ms(attribute_value(xml, "SystemTime")?)?;
    let event_id = event_id(xml)?;
    Some((
        event_ms,
        WindowsGpuEvent {
            event_id,
            gpu_bus_id: gpu_bus_id(xml),
            is_fecs: xml.contains("FECS"),
            is_tdr: xml.contains("Restarting TDR") || xml.contains("Reset TDR"),
        },
    ))
}

fn event_id(xml: &str) -> Option<u32> {
    let tag_start = xml.find("<EventID")?;
    let value_start = xml[tag_start..].find('>')? + tag_start + 1;
    let value_end = xml[value_start..].find('<')? + value_start;
    xml[value_start..value_end].trim().parse().ok()
}

fn gpu_bus_id(xml: &str) -> Option<u32> {
    let value_start = xml.find("GPUID:")? + "GPUID:".len();
    let value = xml[value_start..].trim_start();
    let value_end = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    value[..value_end].parse().ok()
}

fn attribute_value<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let value_start = xml.find(name)? + name.len();
    let after_name = xml[value_start..].trim_start();
    let after_equals = after_name.strip_prefix('=')?.trim_start();
    let quote = after_equals.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let raw_value = &after_equals[quote.len_utf8()..];
    let value_end = raw_value.find(quote)?;
    Some(&raw_value[..value_end])
}

fn parse_windows_event_time_ms(value: &str) -> Option<i128> {
    let year = parse_digits(value, 0, 4)? as i32;
    expect_byte(value, 4, b'-')?;
    let month = parse_digits(value, 5, 7)?;
    expect_byte(value, 7, b'-')?;
    let day = parse_digits(value, 8, 10)?;
    expect_byte(value, 10, b'T')?;
    let hour = parse_digits(value, 11, 13)?;
    expect_byte(value, 13, b':')?;
    let minute = parse_digits(value, 14, 16)?;
    expect_byte(value, 16, b':')?;
    let second = parse_digits(value, 17, 19)?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut millis = 0i128;
    let tail = value.get(19..)?;
    let timezone = if let Some(fraction) = tail.strip_prefix('.') {
        let digit_count = fraction
            .bytes()
            .take_while(u8::is_ascii_digit)
            .take(3)
            .count();
        let digits = &fraction[..digit_count];
        millis = match digit_count {
            0 => 0,
            1 => digits.parse::<i128>().ok()? * 100,
            2 => digits.parse::<i128>().ok()? * 10,
            _ => digits.parse::<i128>().ok()?,
        };
        &fraction[fraction.bytes().take_while(u8::is_ascii_digit).count()..]
    } else {
        tail
    };
    if timezone != "Z" {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let seconds =
        days as i128 * 86_400 + hour as i128 * 3_600 + minute as i128 * 60 + second as i128;
    Some(seconds * 1_000 + millis)
}

fn parse_digits(value: &str, start: usize, end: usize) -> Option<u32> {
    let digits = value.get(start..end)?;
    if digits.bytes().all(|ch| ch.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

fn expect_byte(value: &str, index: usize, expected: u8) -> Option<()> {
    (value.as_bytes().get(index).copied()? == expected).then_some(())
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML_PREFIX: &str = r#"<Event><System><Provider Name="nvlddmkm"/><EventID>"#;

    fn xml(event_id: u32, system_time: &str, data: &str) -> String {
        format!(
            r#"{XML_PREFIX}{event_id}</EventID><TimeCreated SystemTime="{system_time}"/></System><EventData><Data>{data}</Data></EventData></Event>"#
        )
    }

    #[test]
    fn parses_gpuid_from_event_xml() {
        let (_, event) =
            event_from_xml(&xml(13, "2026-07-25T11:22:33.4567890Z", "GPUID: 512 FECS"))
                .expect("event should parse");

        assert_eq!(event.event_id, 13);
        assert_eq!(event.gpu_bus_id, Some(512));
        assert!(event.is_fecs);
        assert!(!event.is_tdr);
    }

    #[test]
    fn missing_gpuid_remains_system_wide_event() {
        let (_, event) = event_from_xml(&xml(4101, "2026-07-25T11:22:33Z", r"\Device\Video3"))
            .expect("event should parse");

        assert_eq!(event.gpu_bus_id, None);
        assert!(!event.is_fecs);
        assert!(!event.is_tdr);
    }

    #[test]
    fn detects_tdr_variants() {
        let (_, restarting) = event_from_xml(&xml(
            14,
            "2026-07-25T11:22:33.4Z",
            "Restarting TDR for GPUID: 256",
        ))
        .expect("event should parse");
        let (_, reset) = event_from_xml(&xml(
            153,
            "2026-07-25T11:22:33.45Z",
            "Reset TDR for GPUID: 256",
        ))
        .expect("event should parse");

        assert!(restarting.is_tdr);
        assert!(reset.is_tdr);
    }

    #[test]
    fn non_critical_event_is_neither_fecs_nor_tdr() {
        let (_, event) = event_from_xml(&xml(10110, "2026-07-25T11:22:33.456Z", "Device problem"))
            .expect("event should parse");

        assert!(!event.is_fecs);
        assert!(!event.is_tdr);
    }

    #[test]
    fn parses_windows_event_timestamp_to_unix_millis() {
        assert_eq!(
            parse_windows_event_time_ms("1970-01-01T00:00:01.2345678Z"),
            Some(1234)
        );
        assert_eq!(
            parse_windows_event_time_ms("1970-01-02T00:00:00Z"),
            Some(86_400_000)
        );
    }
}
