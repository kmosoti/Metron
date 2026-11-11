# Admission model checks

These public models check reservation arithmetic and host-grant intersection.
They do not certify the Rust implementation, SQL transaction ordering, sandbox
or an agent's completion claims. Continue running the runtime's overflow,
concurrent admission, authorization and crash/recovery tests.

Use the pinned Z3/Lean tools from the local toolkit inventory:

```sh
z3 reservation.smt2                 # must print unsat
z3 reservation-mutant.smt2          # must print sat and a counterexample
/absolute/pinned/lean Admission.lean # must succeed, with no nonstandard axioms
/absolute/pinned/lean FalseAdmission.lean # must fail
```

Keep generated Lean outputs on SSD scratch using `-o /scratch/Admission.olean`.
Do not replace a failed check with a sorry/admit, disable kernel checking, or
trust a candidate-provided compiled cache. An ordinary coding task is not
required to carry a formal proof. Model-proposed SMT/Lean calls will use the
same inputs/results through sandboxed, admitted runtime adapters in alpha.1.
