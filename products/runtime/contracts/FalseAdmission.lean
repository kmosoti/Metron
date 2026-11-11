import Std

-- Negative control. This file must FAIL to check: it omits reserved from the
-- admission premise. There is intentionally no sorry/admit or custom axiom.
theorem weakened_reservation (spent reserved requested limit : Nat)
    (admitted : spent + requested ≤ limit) :
    spent + (reserved + requested) ≤ limit := by
  simpa only [Nat.add_assoc] using admitted
