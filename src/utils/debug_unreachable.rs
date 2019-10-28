#[cfg(release)]
pub fn debug_unreachable() -> ! {
    unsafe {
        std::hint::unreachable_unchecked();
    }
}

#[cfg(not(release))]
pub fn debug_unreachable() -> ! {
    eprintln!("debug_unreachable reached unreachable code. This is UB in release mode.");
    std::process::exit(0x75686f68);
}
