use crate::autoscan_config::{FixResultConfig, VfpExportConfig};
// oc_set_function
use crate::error::Error;
use crate::nvidia_gpu_type::{GpuType, fetch_gpu_type};
use crate::oc_get_set_function_nvapi::{get_gpu_tdp_temp_limit, set_pstate_base_voltage};
#[cfg(all(not(windows), not(target_os = "linux")))]
use crate::platform::panic_windows_only;
use csv::{ReaderBuilder, StringRecord, WriterBuilder};
use num_traits::abs;
use nvapi_hi::{
    CoolerPolicy, CoolerSettings, FanCoolerId, Kilohertz, KilohertzDelta, Microvolts, Percentage,
    SensorThrottle,
};
use nvapi_hi::{Gpu, VfPoint, VfpDeltas, VfpTable};
use std::cmp::min;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::process::Child;

#[cfg(any(windows, target_os = "linux"))]
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;
use std::{fs, iter};
// Adjust imports as needed

fn is_std(str: &str) -> bool {
    str == "-"
}

#[cfg(windows)]
fn spawn_dynamic_load_process() -> Result<Child, Error> {
    let repo_root = env!("CARGO_MANIFEST_DIR");
    Command::new("cmd")
        .args(["/C", r"test\dyn_load_export_windows.bat"])
        .current_dir(repo_root)
        .spawn()
        .map_err(|e| Error::Custom(format!("Failed to start Windows load process: {}", e)))
}

#[cfg(target_os = "linux")]
fn spawn_dynamic_load_process() -> Result<Child, Error> {
    let repo_root = env!("CARGO_MANIFEST_DIR");
    Command::new("bash")
        .arg("./test/dyn_load_export_opencl_linux.sh")
        .current_dir(repo_root)
        .spawn()
        .map_err(|e| {
            Error::Custom(format!(
                "Failed to start Linux load process with test/test_opencl_linux.sh load 10: {}",
                e
            ))
        })
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn spawn_dynamic_load_process() -> Result<Child, Error> {
    panic_windows_only("dynamic VFP export")
}

pub fn export_single_point(point: VfPoint, matches: &clap::ArgMatches) -> Result<(), Error> {
    let file_path = matches
        .get_one::<String>("output")
        .map(|s| s.as_str())
        .unwrap();
    let init_path = matches
        .get_one::<String>("initcsv")
        .map(|s| s.as_str())
        .unwrap();

    // Check if the destination file exists
    if !Path::new(file_path).exists() {
        // Copy the file if it doesn't exist
        fs::copy(init_path, file_path)?;
        println!("temporary output file generated successfully!");
        let output_2bcleared = File::open(file_path)?;
        let reader = BufReader::new(output_2bcleared);
        let mut line_number = 1;
        let mut modified_lines = Vec::new();

        // Iterate over each line in the file
        for line in reader.lines() {
            let line = line?; // Get the line as a String
            if line_number == 1 {
                modified_lines.push(line);
                line_number += 1;
                continue;
            }
            let mut columns: Vec<&str> = line.split(',').collect();

            // Remove the 2nd and 3rd values (index 1 and 2)
            if columns.len() > 2 {
                columns[1] = ""; // Clear second value
                columns[2] = ""; // Clear third value
            }
            modified_lines.push(columns.join(",")); // Store the modified line
            line_number += 1;
        }

        // Write the modified content back to the file
        let mut output_file = File::create(file_path)?;
        for line in modified_lines {
            writeln!(output_file, "{}", line)?;
        }
        println!("File updated successfully!");
    } else {
        println!("using existing temp file...");
    }

    // Open the output file for reading and writing
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let mut record_lines = Vec::new();

    // Convert to String and store in variables
    let new_voltage = point.voltage.0;
    let new_delta = point.delta.0;

    for line in reader.lines() {
        let line = line?;
        let mut columns: Vec<String> = line.split(',').map(str::to_owned).collect();
        let voltage = new_voltage.to_string();
        let delta = new_delta.to_string();

        // Check if the row matches
        if columns.first().map(String::as_str) == Some(voltage.as_str()) && columns.len() > 3 {
            // Convert parts[2] and parts[3] to integers safely before mutating columns[2].
            let col3_value: i32 = columns[3].parse().unwrap_or(0);
            let sum = new_delta + col3_value;
            columns[1] = sum.to_string();
            columns[2] = delta;
        }

        record_lines.push(columns.join(",")); // Store modified line
    }

    // Write the updated content back to the file
    let mut output_file = File::create(file_path)?;
    for line in record_lines {
        writeln!(output_file, "{}", line)?;
    }
    println!(
        "Updated row {}μV with delta = {} kHz",
        new_voltage,
        new_delta.clone()
    );

    Ok(())
}

fn export_vfp<W: Write, I: Iterator<Item = VfPoint>>(
    write: W,
    points: I,
    delimiter: u8,
) -> io::Result<()> {
    let mut w = WriterBuilder::new().delimiter(delimiter).from_writer(write);
    let _: () = for point in points {
        w.serialize(point)?;
    };
    Ok(())
}

fn collect_vf_points(vfp: VfpTable, deltas: VfpDeltas, export_memory: bool) -> Vec<VfPoint> {
    let (points, deltas) = if export_memory {
        (vfp.memory, deltas.memory)
    } else {
        (vfp.graphics, deltas.graphics)
    };

    points
        .into_iter()
        .zip(deltas)
        .map(|((i0, mut point), (i1, delta))| {
            assert_eq!(i0, i1);
            if export_memory {
                point.voltage = Microvolts(point.voltage.0 * 2);
            } // video memory voltage should × 2
            VfPoint::new(point, delta)
        })
        .collect()
}

fn extract_default_frequencies(file_path: &str, legacy_flag: bool) -> Result<Vec<u32>, Error> {
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .from_path(file_path)?;
    let mut default_frequencies_load = Vec::new();

    for result in rdr.records() {
        let record = result?;
        let default_frequency_load: u32;

        if legacy_flag {
            default_frequency_load = record
                .get(1)
                .ok_or_else(|| {
                    crate::error::Error::Custom("row too short: missing column 1".into())
                })?
                .parse()?;
        }
        // Read only frequency column
        else {
            default_frequency_load = record
                .get(3)
                .ok_or_else(|| {
                    crate::error::Error::Custom("row too short: missing column 3".into())
                })?
                .parse()?;
        }
        // Read only default_frequency column

        default_frequencies_load.push(default_frequency_load);
    }
    Ok(default_frequencies_load)
}

fn update_csv_with_load_and_margin(
    file_path: &str,
    default_frequencies: Vec<u32>,
    default_frequencies_load: Vec<u32>,
    minimum_delta_core_freq_step: i32,
    legacy_flag: bool,
) -> Result<(), Error> {
    let dest_path = std::path::Path::new(file_path);
    let tmp_name = format!(
        ".{}.{}.tmp",
        dest_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("tmp"),
        std::process::id()
    );
    let tmp_path = dest_path.parent().unwrap_or(dest_path).join(&tmp_name);

    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .from_path(file_path)?;
    let mut wtr = WriterBuilder::new()
        .has_headers(true)
        .from_writer(File::create(&tmp_path)?);

    let mut index = 0;
    let headers = StringRecord::from(vec![
        "voltage",
        "frequency",
        "delta",
        "default_frequency",
        "default_frequency_load",
        "margin",
        "margin_bin",
    ]);
    wtr.write_record(&headers)?;
    for result in rdr.records() {
        let record = result?;
        let voltage = &record[0];
        let frequency = &record[1];
        let delta = &record[2];
        let default_frequency = default_frequencies.get(index).cloned().unwrap_or(0);
        let margin: i32;
        let default_frequency_load = default_frequencies_load.get(index).cloned().unwrap_or(0);

        // Get the corresponding load frequency
        if legacy_flag {
            margin = default_frequency_load as i32 - frequency.parse::<i32>()?;
        } else {
            margin = default_frequency_load as i32 - default_frequency as i32;
        }

        let margin_bin = (margin as f32 / minimum_delta_core_freq_step as f32).round() as i32;
        // Write updated row
        wtr.write_record([
            voltage,
            frequency,
            delta,
            &default_frequency.to_string(),
            &default_frequency_load.to_string(),
            &margin.to_string(),
            &margin_bin.to_string(),
        ])?;
        index += 1;
    }

    wtr.flush()?;

    // Replace original file with updated file.
    // On Windows, rename fails when the destination already exists; fall back to
    // an explicit remove-then-rename so the tmp file is never silently lost.
    if let Err(rename_err) = fs::rename(&tmp_path, file_path) {
        if let Err(remove_err) = fs::remove_file(file_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(remove_err.into());
        }
        fs::rename(&tmp_path, file_path).map_err(|_| rename_err)?;
    }

    Ok(())
}

/// Add a 4th column to the exported VFP CSV: column2 - column3 for legacy quick export
/// Assumes the CSV has a header row and 3 columns initially.
pub fn patch_vfp_csv_add_column_diff(path: &str, delimiter: u8) -> Result<(), Error> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .delimiter(delimiter)
        .from_reader(reader);

    let headers = csv_reader.headers()?.clone();
    let mut records: Vec<Vec<String>> = Vec::new();
    records.push(headers.iter().map(|s| s.to_string()).collect());

    for result in csv_reader.records() {
        let record = result?;
        let mut row: Vec<String> = record.iter().map(|s| s.to_string()).collect();

        if row.len() >= 4 {
            let freq: i32 = row[1].parse().unwrap_or(0);
            let delta: i32 = row[2].parse().unwrap_or(0);
            row[3] = (freq - delta).to_string();
        } else {
            row.push("".to_string());
        } // pad if row is short
        records.push(row);
    }

    // Overwrite the original file
    let file = OpenOptions::new().write(true).truncate(true).open(path)?;
    let writer = BufWriter::new(file);
    let mut csv_writer = WriterBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .from_writer(writer);

    for row in records {
        csv_writer.write_record(&row)?;
    }
    csv_writer.flush()?;

    Ok(())
}

pub fn handle_vfp_export(gpu: &Gpu, matches: &clap::ArgMatches) -> Result<(), Error> {
    let cfg = VfpExportConfig::from_matches(matches);
    let delimiter = cfg.delimiter;
    let output = cfg.output.as_str();
    let export_memory = cfg.export_memory;

    if !cfg.dynamic_check {
        println!("Warning! Disabling dynamic check may generate unstable scan result!")
    }

    let info = gpu.info()?;
    let gpu_type = fetch_gpu_type(&info).unwrap_or(GpuType::Unknown);
    let minimum_delta_core_freq_step = gpu_type.minimum_freq_step_khz();
    let max_q_flag = gpu_type.is_maxq();
    let legacy_vfp_flag = gpu_type.is_legacy_vfp();

    let status = gpu.status()?;
    let settings = gpu.settings()?;

    let points = collect_vf_points(
        status.vfp.ok_or(Error::VfpUnsupported)?,
        settings.vfp.ok_or(Error::VfpUnsupported)?,
        export_memory,
    )
    .into_iter();

    if is_std(output) {
        export_vfp(io::stdout(), points, delimiter)
    } else {
        export_vfp(File::create(output)?, points, delimiter)
    }?;

    if cfg.dynamic {
        if let Err(e) = apply_autoscan_profile(gpu, matches, 30) {
            eprintln!(
                "apply_autoscan_profile failed: {:?}, continuing export...",
                e
            );
        }
        // lowest all fan to maximize temp-related dynamic V-F curve effect

        // Run load process (apply GPU load)
        let mut child = spawn_dynamic_load_process()?;
        sleep(Duration::from_secs(45));
        //too short duration may result in unstable dynamic result...

        let status = gpu.status()?;
        let settings = gpu.settings()?;
        let points_load = collect_vf_points(
            status.vfp.ok_or(Error::VfpUnsupported)?,
            settings.vfp.ok_or(Error::VfpUnsupported)?,
            export_memory,
        )
        .into_iter();

        // Export the load-default frequency to a temporary file
        let temp_file = "./ws/temp_load.csv";
        export_vfp(File::create(temp_file)?, points_load, delimiter)?;

        // Step 4: Kill the load process
        if let Err(e) = child.kill() {
            return Err(Error::Custom(format!(
                "Failed to terminate load process: {}",
                e
            )));
        }

        // Extract only default_frequency column from temp file
        let default_frequencies = extract_default_frequencies(output, legacy_vfp_flag)?;
        let default_frequencies_load = extract_default_frequencies(temp_file, legacy_vfp_flag)?;

        // Update original CSV with the new columns
        update_csv_with_load_and_margin(
            output,
            default_frequencies,
            default_frequencies_load,
            minimum_delta_core_freq_step,
            legacy_vfp_flag,
        )?;

        // Remove the temporary file
        fs::remove_file(temp_file)?;

        if max_q_flag {
            let threshold = if gpu_type.is_maxq() && matches!(gpu_type, GpuType::Mobile50Series) {
                100
            } else {
                15
            };
            if !cfg.dynamic_check || check_margin_column(output, threshold)? {
                println!("dynamic test result is reasonable!")
            } else {
                fs::remove_file(output)?;
                return Err(Error::Str(
                    "dynamic test failed! Please quit all GPU-related programs before testing...",
                ));
            }
        }
    } else if legacy_vfp_flag {
        patch_vfp_csv_add_column_diff(output, delimiter)?;
    }
    Ok(())
}

pub fn check_margin_column(file_path: &str, threshold: i32) -> Result<bool, Error> {
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .from_path(file_path)?;

    for result in rdr.records() {
        let record = result?;
        if let Some(value) = record.get(6) {
            // 7th column (0-based index)
            if abs(value.parse::<i32>().unwrap_or(0)) > threshold {
                return Ok(true); // Found a value with absolute value greater than threshold
            }
        }
    }

    Ok(false) // No value with absolute value > threshold found
}

pub fn handle_vfp_import(gpu: &Gpu, matches: &clap::ArgMatches) -> Result<(), Error> {
    let delimiter = if matches.get_flag("tabs") {
        b'\t'
    } else {
        b','
    };
    let input = matches
        .get_one::<String>("input")
        .map(|s| s.as_str())
        .unwrap();
    let status = gpu.status()?;
    let vfp = status.vfp.ok_or(Error::VfpUnsupported)?.graphics;

    fn import<R: io::Read>(read: R, delimiter: u8) -> Result<Vec<VfPoint>, csv::Error> {
        let mut csv = ReaderBuilder::new().delimiter(delimiter).from_reader(read);
        let de = csv.deserialize();
        de.collect()
    }

    let input = if is_std(input) {
        import(io::stdin(), delimiter)
    } else {
        import(File::open(input)?, delimiter)
    }
    .map_err(io::Error::from)?;

    gpu.set_vfp(
        input.into_iter().filter_map(|point| {
            vfp.iter()
                .find(|&(_, v)| v.voltage == point.voltage)
                .map(|(&i, _)| (i, point.delta))
        }),
        iter::empty(),
    )?;
    Ok(())
}

// oc_profile_function.rs

fn linear_interpolate(
    v1: u32,
    d1: i32,
    v2: u32,
    d2: i32,
    current_v: u32,
    delta_step: i32,
) -> Result<i32, crate::error::Error> {
    if v1 == v2 {
        let mid = ((d1 as i64 + d2 as i64) / 2) as i32;
        return Ok(mid);
    }
    let (lo_v, lo_d, hi_v, hi_d) = if v1 <= v2 {
        (v1, d1, v2, d2)
    } else {
        (v2, d2, v1, d1)
    };
    if current_v < lo_v || current_v > hi_v {
        return Err(crate::error::Error::Custom(format!(
            "linear_interpolate: current_v {} out of range [{}, {}]",
            current_v, lo_v, hi_v
        )));
    }
    let ratio = (current_v - lo_v) as f64 / (hi_v - lo_v) as f64;
    let interpolated = (hi_d as f64 - lo_d as f64) * ratio + lo_d as f64;
    let rounded = if delta_step != 0 && interpolated >= delta_step as f64 {
        (interpolated / delta_step as f64).floor() * delta_step as f64
    } else {
        0.0
    };
    Ok(rounded as i32)
}

fn get_key_points_indices(
    lines: &[Vec<String>],
) -> Result<(usize, usize, usize, usize), crate::error::Error> {
    let mut key_indices = Vec::new();

    for (i, columns) in lines.iter().enumerate() {
        let freq = columns
            .get(1)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let delta = columns
            .get(2)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        if freq != 0 && delta != 0 {
            key_indices.push(i);
            if key_indices.len() == 4 {
                break;
            }
        }
    }
    if key_indices.len() != 4 {
        return Err(crate::error::Error::Custom(format!(
            "expected 4 key points, found {}",
            key_indices.len()
        )));
    }
    Ok((
        key_indices[0],
        key_indices[1],
        key_indices[2],
        key_indices[3],
    ))
}

fn parse_col<T: std::str::FromStr>(
    row: &[String],
    idx: usize,
    what: &str,
) -> Result<T, crate::error::Error>
where
    T::Err: std::fmt::Display,
{
    row.get(idx)
        .ok_or_else(|| crate::error::Error::Custom(format!("missing column {} ({})", idx, what)))?
        .parse::<T>()
        .map_err(|e| crate::error::Error::Custom(format!("column {} ({}): {}", idx, what, e)))
}

fn interpolate_deltas(
    lines: &mut [Vec<String>],
    minimum_delta_step: i32,
    maxq_flag: bool,
) -> Result<(), crate::error::Error> {
    let (p1_idx, p2_idx, p3_idx, p4_idx) = get_key_points_indices(lines)?;

    let p1_d;
    let p2_d;
    let p3_d;
    let p4_d;

    let p1_v = parse_col::<u32>(&lines[p1_idx], 0, "voltage")?;
    let p2_v = parse_col::<u32>(&lines[p2_idx], 0, "voltage")?;
    let p3_v = parse_col::<u32>(&lines[p3_idx], 0, "voltage")?;
    let p4_v = parse_col::<u32>(&lines[p4_idx], 0, "voltage")?;

    if maxq_flag {
        p1_d = parse_col::<i32>(&lines[p1_idx], 2, "delta")? - minimum_delta_step;
        p2_d = parse_col::<i32>(&lines[p2_idx], 2, "delta")? - minimum_delta_step;
        p3_d = parse_col::<i32>(&lines[p3_idx], 2, "delta")? - 2 * minimum_delta_step;
        p4_d = parse_col::<i32>(&lines[p4_idx], 2, "delta")? - 2 * minimum_delta_step;
    } else {
        p1_d = parse_col::<i32>(&lines[p1_idx], 2, "delta")?;
        p2_d = parse_col::<i32>(&lines[p2_idx], 2, "delta")?;
        p3_d = parse_col::<i32>(&lines[p3_idx], 2, "delta")?;
        p4_d = parse_col::<i32>(&lines[p4_idx], 2, "delta")?;
    }

    for i in 0..lines.len() {
        let current_v = parse_col::<u32>(&lines[i], 0, "voltage")?;
        let stair_inferred = min(p2_idx - p1_idx, p3_idx - p2_idx);

        let new_delta = if i < p1_idx {
            p1_d
        } else if i < p2_idx && stair_inferred == p2_idx - p1_idx && maxq_flag {
            min(p1_d, p2_d)
        } else if i < p2_idx {
            linear_interpolate(p1_v, p1_d, p2_v, p2_d, current_v, minimum_delta_step)?
        } else if i < p3_idx && stair_inferred == p3_idx - p2_idx && maxq_flag {
            min(p2_d, p3_d)
        } else if i < p3_idx {
            linear_interpolate(p2_v, p2_d, p3_v, p3_d, current_v, minimum_delta_step)?
        } else if i < p4_idx {
            linear_interpolate(p3_v, p3_d, p4_v, p4_d, current_v, minimum_delta_step)?
        } else {
            p4_d
        };

        lines[i][2] = new_delta.to_string();
    }
    Ok(())
}

pub fn fix_result(gpu: &Gpu, matches: &clap::ArgMatches) -> Result<(), Error> {
    let cfg = FixResultConfig::from_matches(matches)?;

    if cfg.is_ultrafast {
        println!("Ultrafast mode interpolation active...");
    }

    let mut sum_f: u64 = 0;
    let mut sum_df: u64 = 0;

    let info = gpu.info()?;
    let gpu_type = fetch_gpu_type(&info).unwrap_or(GpuType::Unknown);
    let minimum_delta_core_freq_step = gpu_type.minimum_freq_step_khz();
    let maxq_flag = gpu_type.is_maxq();

    // Copy the file if it doesn't exist
    fs::copy(&cfg.vfpath, &cfg.output)?;
    println!("intermediate output file generated successfully!");

    let reader = BufReader::new(File::open(&cfg.output)?);
    let mut modified_lines = Vec::new();
    let mut all_columns: Vec<Vec<String>> = Vec::new();
    let mut line_number = 1;
    for line in reader.lines() {
        let line = line?;
        if line_number == 1 {
            modified_lines.push(line);
            line_number += 1;
            continue;
        }
        let columns: Vec<String> = line.split(',').map(|s| s.to_string()).collect();
        all_columns.push(columns);
        line_number += 1;
    }
    // interpolate when ultrafast
    if cfg.is_ultrafast {
        interpolate_deltas(&mut all_columns, minimum_delta_core_freq_step, maxq_flag)?;
    }

    for columns in &mut all_columns {
        let v = columns
            .first()
            .ok_or("No data in csv frequency")?
            .parse::<u32>()?;

        let mut current_freq = columns
            .get(1)
            .filter(|s| !s.is_empty()) // Ensure it's not an empty string
            .map(|s| s.parse::<u32>().unwrap_or(0)) // Parse, default to 0 if it fails
            .unwrap_or(0); // Default to 0 if index 1 is missing
        let mut delta = columns
            .get(2)
            .filter(|s| !s.is_empty()) // Ensure it's not an empty string
            .map(|s| s.parse::<i32>().unwrap_or(0)) // Parse, default to 0 if it fails
            .unwrap_or(0); // Default to 0 if index 2 is missing;

        let default_freq = columns
            .get(3)
            .ok_or("No data in csv frequency")?
            .parse::<u32>()?;
        let default_load_freq = columns
            .get(4)
            .ok_or("No data in csv frequency")?
            .parse::<u32>()?;
        let margin_freq = columns
            .get(5)
            .ok_or("No data in csv frequency")?
            .parse::<i32>()?;
        let margin_bin = columns
            .get(6)
            .ok_or("No data in csv frequency")?
            .parse::<i32>()?;

        sum_df += default_freq as u64;

        if margin_bin > 5 {
            delta -= minimum_delta_core_freq_step * (5 + cfg.minus_bin);
        } else if abs(margin_bin) < 2 {
            delta -= minimum_delta_core_freq_step * (1 + cfg.minus_bin);
        } else {
            delta -= minimum_delta_core_freq_step * (abs(margin_bin) + 1 + cfg.minus_bin);
        }

        if delta < 0 {
            delta = 0
        };
        if current_freq != default_freq + (delta as u32) {
            current_freq = default_freq + (delta as u32)
        };
        sum_f += current_freq as u64;

        let new_line = format!(
            "{},{},{},{},{},{},{}",
            v, current_freq, delta, default_freq, default_load_freq, margin_freq, margin_bin
        );
        modified_lines.push(new_line);
    }

    // Write the modified content back to the file
    let mut output_file = File::create(&cfg.output)?;
    for line in modified_lines {
        writeln!(output_file, "{}", line)?;
    }
    println!("File updated successfully!");
    println!("This GPU has a SP score of {}", sum_f * 100 / sum_df);

    Ok(())
}

pub fn check_voltage_points(
    log_filename: &str,
) -> io::Result<
    Option<(
        i32,
        i32, // lower & upper voltage point
        Option<usize>,
        Option<usize>,
        Option<usize>,
        Option<usize>, // key points
    )>,
> {
    // Helper function to extract voltage value from a log line
    fn extract_voltage_point(line: &str) -> Option<i32> {
        line.split_whitespace()
            .filter_map(|word| word.parse::<i32>().ok()) // Try to parse integers
            .next() // Take the first one found
    }

    // Helper function to extract four usize values from a line
    fn extract_key_points(line: &str) -> Option<(usize, usize, usize, usize)> {
        let numbers: Vec<usize> = line
            .split(|c: char| !c.is_ascii_digit()) // Split on non-numeric characters
            .filter_map(|num| num.parse::<usize>().ok()) // Parse integers
            .collect();

        if numbers.len() == 4 {
            Some((numbers[0], numbers[1], numbers[2], numbers[3]))
        } else {
            None
        }
    }

    let path = Path::new(log_filename);

    // If the log file doesn't exist, scanning should be initialized
    if !path.exists() {
        return Ok(None);
    }

    let file = File::open(log_filename)?;
    let reader = BufReader::new(file);

    let mut min_voltage_point: Option<i32> = None;
    let mut max_voltage_point: Option<i32> = None;
    let mut key_points: Option<(usize, usize, usize, usize)> = None;

    for line in reader.lines() {
        let line = line?; // Unwrap line safely

        // Check for minimum voltage point
        // Minimum voltage point
        if line.contains("minimum_voltage_point") {
            min_voltage_point = extract_voltage_point(&line);
        }

        if line.contains("maximum_voltage_point") {
            max_voltage_point = extract_voltage_point(&line);
        }

        if line.contains("key points detected:") {
            key_points = extract_key_points(&line);
        }
    }

    // Return lower/upper voltage if found, with optional key points
    if let (Some(lower), Some(upper)) = (min_voltage_point, max_voltage_point) {
        let (p1, p2, p3, p4) = key_points.unwrap_or((0, 0, 0, 0)); // fallback to 0 if missing
        Ok(Some((lower, upper, Some(p1), Some(p2), Some(p3), Some(p4))))
    } else {
        Ok(None)
    }
}

fn extract_value(line: &str, pattern: &str) -> Option<i32> {
    line.split(pattern)
        .nth(1) // Get the part after the pattern
        .and_then(|s| s.split_whitespace().next()) // Get the next word (numeric value)
        .and_then(|s| s.trim_matches(|c| c == '.' || c == ',').parse::<i32>().ok())
    // Parse as i32
}

fn extract_value_f64(line: &str, pattern: &str) -> Option<f64> {
    line.split(pattern)
        .nth(1) // Get the part after the pattern
        .and_then(|s| s.split_whitespace().next()) // Get the next word (numeric value)
        .and_then(|s| s.trim_matches(|c| c == ',').parse::<f64>().ok()) // Parse as f64
}

pub fn break_point_continue(
    log_filename: &str,
    testing_step: usize,
) -> io::Result<(Option<f64>, Option<f64>, Option<usize>, Option<bool>)> {
    let file = File::open(log_filename)?;
    let reader = BufReader::new(file);
    // Read all lines into a Vec
    let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;

    let mut last_code_100_freq: Option<f64> = None;
    let mut last_code_0_freq: Option<f64> = None;
    let mut last_voltage_point: Option<usize> = None;

    let mut ultrafast_flag: Option<bool> = None;

    for line in lines.iter().rev() {
        // Reverse iteration over lines

        if line.contains("succeeded") {
            break;
        }

        if line.contains("Scan")
            || line.contains("Finished")
                && (last_voltage_point.is_some()
                    || last_code_100_freq.is_some()
                    || last_code_0_freq.is_some())
        {
            if line.contains("ultrafast") {
                ultrafast_flag = Some(true)
            } else if line.contains("normal") {
                ultrafast_flag = Some(false)
            }
            break;
        }

        if last_voltage_point.is_none() {
            if let Some(point) = extract_value(line, "point: #") {
                last_voltage_point = Some(point as usize);

                if line.contains("Finished") {
                    last_voltage_point = last_voltage_point.map(|v| v + testing_step);
                    break;
                }
            }
        }

        if last_code_100_freq.is_none() {
            if line.contains("Test result is code #0") {
                if let Some(freq) = extract_value_f64(line, "freq_delta: #") {
                    last_code_100_freq = Some(freq);
                }
            }
        }

        if last_code_0_freq.is_none() {
            if line.contains("Test") && !line.contains("Test result is code #0") {
                if let Some(freq) = extract_value_f64(line, "freq_delta: #") {
                    last_code_0_freq = Some(freq);
                }
            }
        }

        if last_voltage_point.is_some()
            && last_code_100_freq.is_some()
            && last_code_0_freq.is_some()
        {
            break; // Stop early if all values are found
        }
    }

    Ok((
        last_code_100_freq,
        last_code_0_freq,
        last_voltage_point,
        ultrafast_flag,
    ))
}

pub fn export_vfp_from_log(matches: &clap::ArgMatches) -> Result<(), Error> {
    let log_filename = matches
        .get_one::<String>("log")
        .map(|s| s.as_str())
        .unwrap();
    let file = File::open(log_filename)?;
    let reader = BufReader::new(file);

    // Read all lines into a Vec
    let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;

    let mut last_code_100_freq: Option<f64> = None;
    let mut last_voltage: Option<f64> = None;
    let mut last_voltage_point: Option<i32> = None;

    for line in lines.iter().rev() {
        // Reverse iteration over lines

        if line.contains("minimum_voltage_point") {
            break;
        }

        if line.contains("Finished") {
            if let Some(point) = extract_value(line, "point: #") {
                last_voltage_point = Some(point);
                last_code_100_freq = None;
            }
        }

        if last_code_100_freq.is_none() && line.contains("Test result is code #100") {
            println!("{}", line);
            if last_voltage_point.is_none() {
                continue;
            }
            if let Some(point) = extract_value(line, "point: #") {
                if last_voltage_point != Some(point) {
                    eprintln!(
                        "Warning: export_vfp_from_log: expected voltage point {:?}, got {} — skipping",
                        last_voltage_point, point
                    );
                    continue;
                }
            }
            if let Some(voltage) = extract_value_f64(line, "voltage: #") {
                last_voltage = Some(voltage);
            }
            if let Some(freq) = extract_value_f64(line, "freq_delta: #") {
                last_code_100_freq = Some(freq);
                export_single_point(
                    VfPoint {
                        voltage: Microvolts((last_voltage.unwrap() * 1000.0) as u32),
                        frequency: Kilohertz(0),
                        delta: KilohertzDelta((last_code_100_freq.unwrap() * 1000.0) as i32),
                        default_frequency: Kilohertz(0),
                    },
                    matches,
                )?;
            }
        }
    }
    Ok(())
}

pub fn key_point_extractor(
    gpus: &[&Gpu],
    point_l: usize,
    point_u: usize,
    file_path: &str,
) -> Result<(usize, usize, usize, usize), Error> {
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .from_path(file_path)?;

    let mut maxq_flag = false;
    for gpu in gpus {
        let info = gpu.info()?;
        let gpu_type = fetch_gpu_type(&info).unwrap_or(GpuType::Unknown);
        maxq_flag = gpu_type.is_maxq();
    }

    if maxq_flag {
        let mut prev_margin_bin = None;
        let mut max_default_freq_diff_row = None;
        let mut max_default_freq_load_diff_row = None;
        let mut max_default_freq_diff = 0;
        let mut max_default_freq_load_diff = 0;
        let mut first_negative_margin_bin_row = None;
        let mut max_negative_margin_row = None;

        let mut records: Vec<(usize, u32, u32, i32)> = Vec::new();

        for (idx, result) in rdr.records().enumerate() {
            let record = result?;
            let voltage: i32 = record[0].parse()?;
            let default_freq: u32 = record[3].parse()?;
            let default_freq_load: u32 = record[4].parse()?;
            let margin_bin: i32 = record[6].parse()?;

            if voltage > 680000 {
                records.push((idx, default_freq, default_freq_load, margin_bin));
            }

            if let Some(prev) = prev_margin_bin {
                if prev == 0 && margin_bin < 0 && first_negative_margin_bin_row.is_none() {
                    first_negative_margin_bin_row = Some(idx - 1);
                }
                if margin_bin < prev {
                    max_negative_margin_row = Some(idx);
                }
            }
            prev_margin_bin = Some(margin_bin);
        }

        if records.is_empty() {
            return Err(crate::error::Error::Custom(
                "key_point_extractor: no records in VFP CSV".into(),
            ));
        }

        for i in 0..records.len() - 1 {
            let (row, default_freq, default_freq_load, _) = records[i];
            let (_, next_default_freq, next_default_freq_load, _) = records[i + 1];
            let default_freq_diff = next_default_freq - default_freq;
            let default_freq_load_diff = next_default_freq_load - default_freq_load;

            if default_freq_diff > max_default_freq_diff {
                max_default_freq_diff = default_freq_diff;
                max_default_freq_diff_row = Some(row);
            }
            if default_freq_load_diff > max_default_freq_load_diff {
                max_default_freq_load_diff = default_freq_load_diff;
                max_default_freq_load_diff_row = Some(row);
            }
        }

        let mut values = [
            max_default_freq_diff_row.unwrap_or(0),
            max_default_freq_load_diff_row.unwrap_or(0),
            first_negative_margin_bin_row.unwrap_or(0),
            max_negative_margin_row.unwrap_or(0),
        ];
        values.sort_unstable(); // Sort the array in ascending order
        Ok((values[0], values[1], values[2], values[3])) // Return values in sorted order
    } else {
        Ok((
            point_l,
            (3 * point_l + point_u) / 4,
            (point_l + 3 * point_u) / 4,
            point_u,
        ))
    }
}

pub fn apply_autoscan_profile(
    gpu: &Gpu,
    matches: &clap::ArgMatches,
    cooler_level: u32,
) -> Result<(), Error> {
    let info = gpu.info()?;
    let gpu_name = &info.name;

    if gpu_name.contains("Laptop") || gpu_name.contains("Device") {
        println!("TDP/Temp/VDDQ control not available on MOBILE chips! Skipping...");
        return Ok(());
    }

    // 根据 GPU 世代选择电压设置方式
    // 900 系（Maxwell，GM 代号）及更早 → 使用 set_pstate_base_voltage（P0 baseVoltages delta）
    // 10 系（Pascal，GP1 代号）及以后  → 使用 set_voltage_boost（VoltRails boost）
    let gpu_type = fetch_gpu_type(&info).unwrap_or(GpuType::Unknown);

    if gpu_type.is_legacy_voltage() {
        // 900 系及更早：通过 SetPstates20 写 P0 baseVoltage delta（最大允许值，即尽量升压）
        // 先读允许范围，再以最大 delta 写入
        let max_delta = {
            use nvapi_hi::PState;
            gpu.inner().pstates().ok().and_then(|ps| {
                ps.pstates
                    .into_iter()
                    .find(|p| p.id == PState::P0)
                    .and_then(|p0| {
                        p0.base_voltages
                            .into_iter()
                            .find(|v| v.voltage_domain == nvapi_hi::nvapi::VoltageDomain::Core)
                            .map(|v| v.voltage_delta.range.max)
                    })
            })
        };
        match max_delta {
            Some(max_uv) => {
                set_pstate_base_voltage(gpu, max_uv, nvapi_hi::PState::P0)?;
                println!(
                    "Successfully set P0 base voltage delta to max +{}μV (legacy GPU).",
                    max_uv.0
                );
            }
            None => {
                return Err(Error::from(
                    "Could not read P0 Core voltage range; cannot apply voltage boost for legacy GPU.",
                ));
            }
        }
    } else {
        // 10 系及以后：使用 VoltRails boost
        gpu.set_voltage_boost(Percentage(100))?;
        println!("Successfully set VDDQ boost to +100% (max allowed V_core in fact).");
    }

    let settings = [
        (
            FanCoolerId::Cooler1,
            CoolerSettings {
                policy: CoolerPolicy::TemperatureContinuous,
                level: Some(Percentage(cooler_level)),
            },
        ),
        (
            FanCoolerId::Cooler2,
            CoolerSettings {
                policy: CoolerPolicy::TemperatureContinuous,
                level: Some(Percentage(cooler_level)),
            },
        ),
    ];

    gpu.set_cooler_levels(settings)?;
    println!("Successfully set Cooler1 and Cooler2 to {}%.", cooler_level);

    match get_gpu_tdp_temp_limit(matches.clone()) {
        Ok((
            _min_tdp_percent,
            _default_tdp_percent,
            _max_tdp_percent,
            _min_temp_lim,
            _default_temp_lim,
            _max_temp_lim,
            mut _pff_curve,
        )) => {
            gpu.set_power_limits(iter::once(_max_tdp_percent))?;
            println!("Successfully set the TDP to {}", _max_tdp_percent);

            for point in _pff_curve.points.iter_mut() {
                point.y = Kilohertz(3456000);
            }

            let temp_limit = SensorThrottle {
                value: _max_temp_lim,
                remove_tdp_limit: true,
                curve: Some(_pff_curve.clone()),
            };

            gpu.set_sensor_limits(iter::once(temp_limit))?;
            println!(
                "Successfully set the Temp_limit to {} and pff-curve to {}",
                _max_temp_lim, _pff_curve
            );
        }
        Err(e) => {
            return Err(Error::from(format!(
                "Failed to set Power and Temp limit: {:?}",
                e
            )));
        }
    }

    Ok(())
}
