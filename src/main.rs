use std::env;
use std::process::{Command, Stdio};
use std::str;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct Config {
    size: String,
    name: String,
    filesystem: String,
    verbose: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            size: String::new(),
            name: "RAMDisk".to_string(),
            filesystem: "apfs".to_string(),
            verbose: false,
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    match parse_args(&args[1..]) {
        Ok(config) => {
            if let Err(e) = create_ramdisk(&config) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            print_usage();
            std::process::exit(1);
        }
    }
}

fn print_usage() {
    println!(r#"
Usage: mkramdisk [OPTIONS] <size> [name]

Create a RAM disk on macOS with specified size and optional name.

Arguments:
    size    Size of RAM disk (e.g., 1G, 512M, 2048K)
            Supports suffixes: K/KB, M/MB, G/GB, T/TB
    name    Optional name for the RAM disk (default: RAMDisk)

Options:
    -f, --format FS     Filesystem format (default: apfs)
                        Supported: apfs, hfs+, fat32, exfat
    -v, --verbose       Show detailed output
    -h, --help         Show this help message

Examples:
    mkramdisk 1G                    # Create 1GB APFS RAM disk named "RAMDisk"
    mkramdisk 512M MyRAM            # Create 512MB APFS RAM disk named "MyRAM"
    mkramdisk -f hfs+ 2G TempDisk   # Create 2GB HFS+ RAM disk named "TempDisk"
    mkramdisk --format fat32 256M   # Create 256MB FAT32 RAM disk
"#);
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut config = Config::default();
    let mut i = 0;
    
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-v" | "--verbose" => {
                config.verbose = true;
                i += 1;
            }
            "-f" | "--format" => {
                if i + 1 >= args.len() {
                    return Err("Format option requires a value".to_string());
                }
                config.filesystem = args[i + 1].clone();
                i += 2;
            }
            arg if arg.starts_with('-') => {
                return Err(format!("Unknown option: {}", arg));
            }
            _ => {
                if config.size.is_empty() {
                    config.size = args[i].clone();
                } else if config.name == "RAMDisk" {
                    config.name = args[i].clone();
                } else {
                    return Err("Too many arguments".to_string());
                }
                i += 1;
            }
        }
    }
    
    if config.size.is_empty() {
        return Err("Size argument is required".to_string());
    }
    
    // Validate filesystem format early
    validate_filesystem(&config.filesystem)?;
    
    // Sanitize volume name
    config.name = sanitize_volume_name(&config.name);
    
    Ok(config)
}

fn validate_filesystem(filesystem: &str) -> Result<(), String> {
    match filesystem.to_lowercase().as_str() {
        "apfs" | "hfs+" | "hfs" | "fat32" | "msdos" | "exfat" => Ok(()),
        _ => Err(format!(
            "Unsupported filesystem: {}\nSupported filesystems: apfs, hfs+, fat32, exfat", 
            filesystem
        )),
    }
}

fn sanitize_volume_name(name: &str) -> String {
    // Remove characters that could cause issues with volume names
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == ' ')
        .collect::<String>()
        .trim()
        .to_string()
}

fn size_to_sectors(size: &str) -> Result<u64, String> {
    let size = size.to_uppercase();
    let (number_str, suffix) = if let Some(pos) = size.find(|c: char| c.is_alphabetic()) {
        (&size[..pos], &size[pos..])
    } else {
        (size.as_str(), "")
    };
    
    let number: u64 = number_str.parse()
        .map_err(|_| format!("Invalid number in size: {}", number_str))?;
    
    if number == 0 {
        return Err("Size cannot be zero".to_string());
    }
    
    let bytes = match suffix {
        "" | "B" => number,
        "K" | "KB" => number.checked_mul(1024)
            .ok_or("Size too large")?,
        "M" | "MB" => number.checked_mul(1024 * 1024)
            .ok_or("Size too large")?,
        "G" | "GB" => number.checked_mul(1024 * 1024 * 1024)
            .ok_or("Size too large")?,
        "T" | "TB" => number.checked_mul(1024 * 1024 * 1024 * 1024)
            .ok_or("Size too large")?,
        _ => return Err(format!("Unknown size suffix: {}", suffix)),
    };
    
    let sectors = bytes / 512;
    if sectors == 0 {
        return Err("Size too small (minimum 512 bytes)".to_string());
    }
    
    Ok(sectors)
}

fn get_diskutil_format(filesystem: &str) -> Result<String, String> {
    match filesystem.to_lowercase().as_str() {
        "apfs" => Ok("APFS".to_string()),
        "hfs+" | "hfs" => Ok("HFS+".to_string()),
        "fat32" | "msdos" => Ok("MS-DOS FAT32".to_string()),
        "exfat" => Ok("ExFAT".to_string()),
        _ => Err(format!("Unsupported filesystem: {}\nSupported filesystems: apfs, hfs+, fat32, exfat", filesystem)),
    }
}

fn log_verbose(config: &Config, message: &str) {
    if config.verbose {
        eprintln!("[INFO] {}", message);
    }
}

fn cleanup_device(device: &str, verbose: bool) {
    if verbose {
        eprintln!("[INFO] Cleaning up device {}...", device);
    }
    let _ = Command::new("hdiutil")
        .args(&["detach", device])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn wait_for_mount(mount_point: &str, max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        if std::path::Path::new(mount_point).exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn create_ramdisk(config: &Config) -> Result<(), String> {
    // Convert size to sectors
    log_verbose(config, &format!("Converting size '{}' to sectors...", config.size));
    let sectors = size_to_sectors(&config.size)?;
    log_verbose(config, &format!("Size: {} = {} sectors", config.size, sectors));
    
    // Check if volume name already exists
    let mount_point = format!("/Volumes/{}", config.name);
    if std::path::Path::new(&mount_point).exists() {
        return Err(format!("Volume '{}' already exists at {}", config.name, mount_point));
    }
    
    // Create the RAM disk
    log_verbose(config, &format!("Creating RAM disk with {} sectors...", sectors));
    let ram_url = format!("ram://{}", sectors);
    
    let output = Command::new("hdiutil")
        .args(&["attach", "-nomount", &ram_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to execute hdiutil: {}", e))?;
    
    if !output.status.success() {
        let stderr = str::from_utf8(&output.stderr).unwrap_or("Unknown error");
        return Err(format!("Failed to create RAM disk: {}", stderr.trim()));
    }
    
    let device = str::from_utf8(&output.stdout)
        .map_err(|_| "Invalid UTF-8 in hdiutil output")?
        .trim()
        .to_string();
    
    if device.is_empty() {
        return Err("No device returned by hdiutil".to_string());
    }
    
    log_verbose(config, &format!("RAM disk device: {}", device));
    
    // Format the RAM disk using diskutil erasevolume (the proper macOS way)
    log_verbose(config, &format!("Formatting RAM disk as {} with name '{}'...", config.filesystem, config.name));
    
    let diskutil_format = get_diskutil_format(&config.filesystem)?;
    
    let format_output = Command::new("diskutil")
        .args(&["erasevolume", &diskutil_format, &config.name, &device])
        .stdout(if config.verbose { Stdio::inherit() } else { Stdio::null() })
        .stderr(if config.verbose { Stdio::inherit() } else { Stdio::piped() })
        .output()
        .map_err(|e| format!("Failed to execute diskutil: {}", e))?;
    
    if !format_output.status.success() {
        cleanup_device(&device, config.verbose);
        let stderr = if config.verbose {
            "Check verbose output above for details".to_string()
        } else {
            str::from_utf8(&format_output.stderr)
                .unwrap_or("Unknown error")
                .trim()
                .to_string()
        };
        return Err(format!("Failed to format RAM disk: {}", stderr));
    }
    
    // Since diskutil erasevolume formats AND mounts, we just need to wait and verify
    log_verbose(config, "Waiting for RAM disk to mount...");
    if !wait_for_mount(&mount_point, 50) { // Wait up to 5 seconds
        cleanup_device(&device, config.verbose);
        return Err("RAM disk was formatted but failed to mount properly".to_string());
    }
    
    // Verify the RAM disk was created and mounted successfully
    if std::path::Path::new(&mount_point).exists() {
        println!("\x1b[1;32m RAM disk created successfully\x1b[0m");
        println!("  Device:     {}", device);
        println!("  Size:       {}", config.size);
        println!("  Filesystem: {}", config.filesystem);
        println!("  Mount point: {}", mount_point);
        println!("  Name:       {}", config.name);
        println!();
        println!("To unmount: \x1b[1mdiskutil unmount \"{}\"\x1b[0m", mount_point);
        println!("To eject:   \x1b[1mhdiutil detach {}\x1b[0m", device);
    } else {
        cleanup_device(&device, config.verbose);
        return Err("RAM disk creation completed but verification failed".to_string());
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_size_to_sectors() {
        assert_eq!(size_to_sectors("1024").unwrap(), 2);
        assert_eq!(size_to_sectors("1K").unwrap(), 2);
        assert_eq!(size_to_sectors("1KB").unwrap(), 2);
        assert_eq!(size_to_sectors("1M").unwrap(), 2048);
        assert_eq!(size_to_sectors("1MB").unwrap(), 2048);
        assert_eq!(size_to_sectors("1G").unwrap(), 2097152);
        assert_eq!(size_to_sectors("1GB").unwrap(), 2097152);
        
        assert!(size_to_sectors("invalid").is_err());
        assert!(size_to_sectors("1X").is_err());
        assert!(size_to_sectors("0").is_err());
    }
    
    #[test]
    fn test_get_diskutil_format() {
        assert_eq!(get_diskutil_format("apfs").unwrap(), "APFS");
        assert_eq!(get_diskutil_format("hfs+").unwrap(), "HFS+");
        assert_eq!(get_diskutil_format("fat32").unwrap(), "MS-DOS FAT32");
        assert_eq!(get_diskutil_format("exfat").unwrap(), "ExFAT");
        
        assert!(get_diskutil_format("invalid").is_err());
    }
    
    #[test]
    fn test_sanitize_volume_name() {
        assert_eq!(sanitize_volume_name("Test Disk"), "Test Disk");
        assert_eq!(sanitize_volume_name("Test/Disk"), "TestDisk");
        assert_eq!(sanitize_volume_name("Test:Disk"), "TestDisk");
        assert_eq!(sanitize_volume_name("Test-Disk_2"), "Test-Disk_2");
    }
    
    #[test]
    fn test_validate_filesystem() {
        assert!(validate_filesystem("apfs").is_ok());
        assert!(validate_filesystem("hfs+").is_ok());
        assert!(validate_filesystem("fat32").is_ok());
        assert!(validate_filesystem("exfat").is_ok());
        assert!(validate_filesystem("invalid").is_err());
    }
}