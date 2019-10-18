#[cfg(release)]
fn debug_unreachable() -> ! {
    unsafe {
        std::hint::unreachable_unchecked();
    }
}

#[cfg(not(release))]
fn debug_unreachable() -> ! {
    unreachable!("debug_unreachable reached unreachable code. This is UB in release mode.");
}
