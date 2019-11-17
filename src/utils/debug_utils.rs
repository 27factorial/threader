/// A function which panics in any mode other than release, but is simply equivalent to
/// [`unreachable_unchecked`](https://doc.rust-lang.org/nightly/std/hint/fn.unreachable_unchecked.html)
/// in release mode. This function should be used with care, and only if you are able to
/// prove that the code is actually unreachable.
///
/// # Examples
///
/// ```
/// use threader::utils::debug_unreachable;
///
/// // In this case, we know that returns_five always returns 5, so we
/// // can safely call debug_unreachable.
/// fn returns_five() -> i32 {
///     5
/// }
///
/// fn main() {
///     let x = returns_five();
///
///     match x {
///         5 => println!("It returned 5"),
///         _ => unsafe { debug_unreachable() },
///     }   
/// }
/// ```
///
/// # Safety
///
/// This function should only be used when you can prove that any branches which call
/// it can not be reached. While it exits the program with a nonzero exit code when not
/// in release mode, it is completely undefined behavior to call it when in release mode.
#[cfg(not(release))]
pub unsafe fn debug_unreachable() -> ! {
    eprintln!("debug_unreachable reached unreachable code. This is UB in release mode.");
    std::process::exit(0x75686f68);
}

#[cfg(release)]
pub unsafe fn debug_unreachable() -> ! {
    std::hint::unreachable_unchecked();
}
