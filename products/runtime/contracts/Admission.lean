import Std

-- These theorems concern explicit models. Rust checked-integer and concurrent
-- storage behavior needs independent executable verification.
namespace Metron

theorem reservation_preserves (spent reserved requested limit : Nat)
    (admitted : spent + reserved + requested ≤ limit) :
    spent + (reserved + requested) ≤ limit := by
  simpa only [Nat.add_assoc] using admitted

def effectiveGrant (host requested : Nat → Bool) (capability : Nat) : Bool :=
  host capability && requested capability

theorem effective_grant_is_host_subset (host requested : Nat → Bool)
    (capability : Nat) (granted : effectiveGrant host requested capability = true) :
    host capability = true := by
  cases h : host capability <;> simp [effectiveGrant, h] at *

#print axioms reservation_preserves
#print axioms effective_grant_is_host_subset
end Metron
