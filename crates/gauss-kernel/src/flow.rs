//! Information-flow `L` lattice and declassification map (Axiom A6).
//!
//! Phase 1 ships:
//!
//! * The `declass : L → K` map as a small per-tenant decision table.
//! * Build-time + runtime antitone verification: `ℓ1 ≤ ℓ2` MUST imply
//!   `declass(ℓ2) ⪯ declass(ℓ1)` (higher taint admits lower capability).
//! * Two stock policies: [`default_declass`] (paper §VI.B-style permissive)
//!   and the strict variant that maps every taint above `User` to `BOTTOM`.

use gauss_core::TaintLabel;

use crate::cap::CapToken;

/// Declassification map `declass : L → K`. Implementations MUST be antitone.
pub trait DeclassMap: Send + Sync {
    /// Return the maximum capability admissible at the given taint level.
    fn declass(&self, taint: TaintLabel) -> CapToken;
}

/// Verify that a `declass` map is antitone over the four canonical taint
/// labels. The total chain has only four labels, so the check is `O(1)`.
///
/// Returns `Ok(())` if antitone; otherwise an error string describing the
/// first violating pair.
///
/// # Errors
/// Returns `Err` if the map is not antitone (a higher taint admits MORE
/// capability than a strictly lower taint).
pub fn verify_antitone<D: DeclassMap + ?Sized>(d: &D) -> Result<(), AntitoneViolation> {
    use TaintLabel::{Adversarial, Trusted, User, Web};
    let labels = [Trusted, User, Web, Adversarial];
    for &lo in &labels {
        for &hi in &labels {
            if lo.leq(hi) && lo != hi {
                let cap_lo = d.declass(lo);
                let cap_hi = d.declass(hi);
                if !cap_hi.leq(cap_lo) {
                    return Err(AntitoneViolation {
                        lower_taint: lo,
                        higher_taint: hi,
                        cap_at_lower: cap_lo,
                        cap_at_higher: cap_hi,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Antitone-violation diagnostic.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AntitoneViolation {
    /// The strictly lower taint.
    pub lower_taint: TaintLabel,
    /// The strictly higher taint.
    pub higher_taint: TaintLabel,
    /// The (correctly) higher capability admitted at the lower taint.
    pub cap_at_lower: CapToken,
    /// The (incorrectly higher) capability admitted at the higher taint.
    pub cap_at_higher: CapToken,
}

impl core::fmt::Display for AntitoneViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "declass not antitone: {:?}→{:?} declass({:?})={:?} but declass({:?})={:?}",
            self.lower_taint,
            self.higher_taint,
            self.lower_taint,
            self.cap_at_lower,
            self.higher_taint,
            self.cap_at_higher,
        )
    }
}

impl std::error::Error for AntitoneViolation {}

/// Stock permissive declass map (paper §VI.B).
///
/// * `Trusted`     → ⊤ (everything)
/// * `User`        → no irreversible / crypto / subprocess
/// * `Web`         → only read-only network + canvas render
/// * `Adversarial` → ⊥
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultDeclass;

impl DeclassMap for DefaultDeclass {
    fn declass(&self, taint: TaintLabel) -> CapToken {
        match taint {
            TaintLabel::Trusted => CapToken::TOP,
            TaintLabel::User => {
                // Drop CRYPTO_SIGN, NETWORK_POST, SUBPROCESS_SPAWN.
                CapToken::TOP.meet(CapToken::from_bits(
                    !(CapToken::CRYPTO_SIGN.bits()
                        | CapToken::NETWORK_POST.bits()
                        | CapToken::SUBPROCESS_SPAWN.bits()),
                ))
            }
            TaintLabel::Web => CapToken::FILESYSTEM_READ
                .join(CapToken::NETWORK_GET)
                .join(CapToken::CANVAS_RENDER),
            TaintLabel::Adversarial => CapToken::BOTTOM,
        }
    }
}

/// Stock strict declass map: anything above `User` admits nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct StrictDeclass;

impl DeclassMap for StrictDeclass {
    fn declass(&self, taint: TaintLabel) -> CapToken {
        match taint {
            TaintLabel::Trusted => CapToken::TOP,
            TaintLabel::User => CapToken::FILESYSTEM_READ
                .join(CapToken::NETWORK_GET)
                .join(CapToken::CANVAS_RENDER),
            TaintLabel::Web | TaintLabel::Adversarial => CapToken::BOTTOM,
        }
    }
}

/// Helper: `default_declass(taint)` — convenience over the stock map.
#[must_use]
pub fn default_declass(taint: TaintLabel) -> CapToken {
    DefaultDeclass.declass(taint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map_is_antitone() {
        verify_antitone(&DefaultDeclass).expect("default declass must be antitone");
    }

    #[test]
    fn strict_map_is_antitone() {
        verify_antitone(&StrictDeclass).expect("strict declass must be antitone");
    }

    #[test]
    fn adversarial_admits_nothing_under_strict() {
        assert_eq!(
            StrictDeclass.declass(TaintLabel::Adversarial),
            CapToken::BOTTOM
        );
        assert_eq!(StrictDeclass.declass(TaintLabel::Web), CapToken::BOTTOM);
    }

    /// A deliberately broken map to exercise the antitone verifier.
    struct BrokenMap;
    impl DeclassMap for BrokenMap {
        fn declass(&self, taint: TaintLabel) -> CapToken {
            match taint {
                TaintLabel::Trusted => CapToken::FILESYSTEM_READ,
                // Bug: User admits MORE than Trusted — violates antitone.
                _ => CapToken::TOP,
            }
        }
    }

    #[test]
    fn verifier_catches_non_antitone_map() {
        let err = verify_antitone(&BrokenMap).expect_err("must catch the violation");
        assert_eq!(err.lower_taint, TaintLabel::Trusted);
        assert_eq!(err.higher_taint, TaintLabel::User);
    }
}
