//! Stable hardware identity for binding a qualification to its execution host.
use std::io::{self, Read};

use metron_protocol::{
    BoundedText, Digest, SqlU64, content_digest,
    inference::{LocalDevice, QualificationReport},
};

use crate::measurement::{cuda_headroom, nvidia_query};

pub struct CpuIdentity {
    pub identity: String,
    pub features: Digest,
}

pub fn cpu_identity() -> io::Result<CpuIdentity> {
    let mut text = String::new();
    std::fs::File::open("/proc/cpuinfo")?
        .take(64 * 1024)
        .read_to_string(&mut text)?;
    let first = text.split("\n\n").next().ok_or_else(unavailable)?;
    let field = |name: &str| {
        first
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(key, value)| (key.trim() == name).then(|| value.trim()))
            .filter(|value| !value.is_empty())
            .ok_or_else(unavailable)
    };
    let identity = format!(
        "{}|{}|{}|{}|{}",
        field("vendor_id")?,
        field("model name")?,
        field("cpu family")?,
        field("model")?,
        field("stepping")?
    );
    let mut features: Vec<_> = field("flags")?.split_whitespace().collect();
    features.sort_unstable();
    features.dedup();
    Ok(CpuIdentity {
        identity,
        features: content_digest(features.join(" ").as_bytes()),
    })
}

fn unavailable() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "hardware identity unavailable")
}

pub fn validate_cpu(expected_identity: &str, expected_features: &Digest) -> io::Result<()> {
    let actual = cpu_identity()?;
    if actual.identity != expected_identity || &actual.features != expected_features {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "CPU identity differs from qualified profile",
        ));
    }
    Ok(())
}

/// Re-query by pinned UUID; reject MIG until its allocation identity is modeled.
pub fn validate_gpu(device: &LocalDevice) -> io::Result<()> {
    let LocalDevice::Cuda {
        uuid,
        model,
        memory_bytes,
        driver,
    } = device
    else {
        return Ok(());
    };
    let uuid = uuid.as_str();
    if !uuid.starts_with("GPU-") || nvidia_query(uuid, "uuid")? != uuid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GPU UUID differs from qualified profile",
        ));
    }
    let mig = nvidia_query(uuid, "mig.mode.current")?;
    if mig != "Disabled" && mig != "N/A" && mig != "[N/A]" {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "MIG GPU identity is not supported",
        ));
    }
    let memory = nvidia_query(uuid, "memory.total")?
        .parse::<u64>()
        .ok()
        .and_then(|m| m.checked_mul(1024 * 1024))
        .ok_or_else(unavailable)?;
    if nvidia_query(uuid, "name")? != model.as_str()
        || nvidia_query(uuid, "driver_version")? != driver.as_str()
        || memory != memory_bytes.get()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GPU identity differs from qualified profile",
        ));
    }
    Ok(())
}

pub fn gpu_identity(selector: &str) -> io::Result<LocalDevice> {
    let uuid = nvidia_query(selector, "uuid")?;
    let model = nvidia_query(&uuid, "name")?;
    let driver = nvidia_query(&uuid, "driver_version")?;
    let bytes = nvidia_query(&uuid, "memory.total")?
        .parse::<u64>()
        .ok()
        .and_then(|v| v.checked_mul(1024 * 1024))
        .ok_or_else(unavailable)?;
    let device = LocalDevice::Cuda {
        uuid: BoundedText::new(uuid).map_err(|_| unavailable())?,
        model: BoundedText::new(model).map_err(|_| unavailable())?,
        driver: BoundedText::new(driver).map_err(|_| unavailable())?,
        memory_bytes: SqlU64::new(bytes).map_err(|_| unavailable())?,
    };
    validate_gpu(&device)?;
    Ok(device)
}

fn required_available(peak: u64) -> io::Result<u64> {
    // peak + ceil(peak / 10) + 1 GiB, without floating point or overflow.
    peak.checked_add(peak / 10 + u64::from(!peak.is_multiple_of(10)))
        .and_then(|n| n.checked_add(1024 * 1024 * 1024))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "qualified resource envelope overflow",
            )
        })
}

pub(crate) fn available_cpu() -> io::Result<u64> {
    let mut text = String::new();
    std::fs::File::open("/proc/meminfo")?
        .take(64 * 1024)
        .read_to_string(&mut text)?;
    let line = text
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))
        .ok_or_else(unavailable)?;
    let mut values = line["MemAvailable:".len()..].split_whitespace();
    let amount = values
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(unavailable)?;
    if values.next() != Some("kB") || values.next().is_some() {
        return Err(unavailable());
    }
    amount.checked_mul(1024).ok_or_else(unavailable)
}

/// Reject missing measurements and insufficient current headroom before worker startup.
pub fn preflight(
    device: &LocalDevice,
    cold: &QualificationReport,
    warm: &QualificationReport,
) -> io::Result<()> {
    let peak = |report: &QualificationReport, gpu: bool| {
        report
            .measurements
            .iter()
            .filter_map(|m| {
                if gpu {
                    m.peak_gpu_bytes
                } else {
                    m.peak_pss_bytes
                }
            })
            .map(|n| n.get())
            .filter(|n| *n > 0)
            .max()
    };
    let cold_pss = peak(cold, false).ok_or_else(unavailable)?;
    let warm_pss = peak(warm, false).ok_or_else(unavailable)?;
    if available_cpu()? < required_available(cold_pss.max(warm_pss))? {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "insufficient memory for qualified worker",
        ));
    }
    if let LocalDevice::Cuda { uuid, .. } = device {
        let cold_gpu = peak(cold, true).ok_or_else(unavailable)?;
        let warm_gpu = peak(warm, true).ok_or_else(unavailable)?;
        if cuda_headroom(uuid.as_str())? < required_available(cold_gpu.max(warm_gpu))? {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "insufficient GPU memory for qualified worker",
            ));
        }
    }
    Ok(())
}

/// Before calibration, the pinned weight size is only an estimate, not a measured peak.
pub fn calibration_preflight(profile: &metron_protocol::LocalProfile) -> io::Result<()> {
    if available_cpu()? < required_available(profile.artifact_bytes.get())? {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "insufficient estimated CPU headroom",
        ));
    }
    if let LocalDevice::Cuda { uuid, .. } = &profile.device
        && cuda_headroom(uuid.as_str())? < required_available(profile.artifact_bytes.get())?
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "insufficient estimated GPU headroom",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_envelope_rounds_up() {
        assert_eq!(required_available(11).unwrap(), 1024 * 1024 * 1024 + 13);
        assert!(required_available(u64::MAX).is_err());
        assert!(available_cpu().unwrap() > 0);
    }
}
