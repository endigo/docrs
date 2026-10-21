//! Per-work-package synthetic documents (each file owned by one package).

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub mod p6;
pub mod p7;

/// Every M4 synthetic document.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    let mut out = p1::documents();
    out.extend(p2::documents());
    out.extend(p3::documents());
    out.extend(p4::documents());
    out.extend(p5::documents());
    out.extend(p6::documents());
    out.extend(p7::documents());
    out
}
