---
id: O-0003
type: observation
status: limited-probes-passed
updated: 2026-11-10
added_by: agent
human_ratified: false
---

# Runtime contract feasibility probes

## Observed

Two separate SQLite connections contended for one remaining budget unit. Each
worker used `BEGIN IMMEDIATE`, a guarded budget update and reservation insert
inside one transaction. Exactly one reservation committed and the other was
denied. A modeled settlement persisted actual usage of two units against the
one-unit budget, retained the receipt and marked the account breached. Further
admission was denied. A repeated lookup of the accepted intent left the accounting unchanged.
A modeled dispatch-to-uncertain transition retained its reservation after closing
and reopening the database.

A Rust library placed the token's private field and probe factory in the policy
module. A separate driver crate compiled and consumed the read-only accessor.
A separate attempt to construct the token's private field failed with E0451.
The successful consumer binary executed its assertion.

```json
{
  "sqlite_version": "3.51.2",
  "rustc": "rustc 1.97.0 (2d8144b78 2026-07-07)",
  "concurrent_workers": 2,
  "budget_units": 1,
  "accepted_reservations": 1,
  "denied_reservations": 1,
  "duplicate_lookup_does_not_charge": true,
  "modeled_uncertain_retains_reservation": true,
  "overrun_receipt_and_actual_spent_retained": true,
  "breached_account_denies_admission": true,
  "external_driver_reads_approved_request": true,
  "external_token_construction_rejected": "E0451",
  "scope": "standalone probes, no product runtime or actual crash test"
}
```

## Scope and interpretation

These probes demonstrate a SQL concurrency guard and a usable Rust module-privacy
pattern. The duplicate lookup and uncertainty transition are modeled bookkeeping.
They exercise neither the future Rust store's retry implementation nor a real
crash or external side effect. The probe factory deliberately returns a scripted
token and implements no host authorization. It is a visibility fixture.

Process-kill recovery, power-loss durability, stale leases, grant revocation,
provider overruns, sandbox escape resistance and full product APIs remain
implementation acceptance obligations. No benchmark or production assurance is
inferred from these small experiments.

## Reproduction

Use Python 3 with its SQLite standard library and the pinned repository Rust
compiler. The following source is the executed probe. Save it as a temporary Python file
and run it from the repository with `python3 probe.py --rustc <rustc-path>`.
The compiler follows the repository toolchain when its Rustup shim is used.
Temporary data is scoped to a temporary directory and the output path can be
set with `--output`.

```python
import argparse
import json
import sqlite3
import subprocess
import tempfile
import threading
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--rustc', default='rustc')
parser.add_argument('--output', default='/tmp/metron-runtime-contract-probe-results.json')
args = parser.parse_args()

with tempfile.TemporaryDirectory(prefix='metron-contract-probe-') as directory:
    root = Path(directory)
    database = root / 'budget.sqlite'
    connection = sqlite3.connect(database)
    connection.executescript('''
        CREATE TABLE budget (
            id INTEGER PRIMARY KEY,
            ceiling INTEGER NOT NULL,
            spent INTEGER NOT NULL CHECK(spent >= 0),
            reserved INTEGER NOT NULL CHECK(reserved >= 0),
            state TEXT NOT NULL
        );
        INSERT INTO budget VALUES (1, 1, 0, 0, 'open');
        CREATE TABLE reservation (
            intent TEXT PRIMARY KEY,
            amount INTEGER NOT NULL,
            state TEXT NOT NULL
        );
        CREATE TABLE receipt (intent TEXT PRIMARY KEY, actual INTEGER NOT NULL);
    ''')
    connection.close()
    barrier = threading.Barrier(2)
    outcomes = []
    errors = []
    result_lock = threading.Lock()

    def worker(intent):
        worker_connection = sqlite3.connect(database, timeout=5)
        try:
            barrier.wait()
            worker_connection.execute('BEGIN IMMEDIATE')
            changed = worker_connection.execute('''
                UPDATE budget SET reserved = reserved + 1
                WHERE id = 1 AND state = 'open'
                    AND spent + reserved + 1 <= ceiling
            ''').rowcount
            if changed:
                worker_connection.execute(
                    'INSERT INTO reservation VALUES (?, 1, ?)',
                    (intent, 'reserved'),
                )
                worker_connection.commit()
            else:
                worker_connection.rollback()
            with result_lock:
                outcomes.append((intent, bool(changed)))
        except Exception as error:
            with result_lock:
                errors.append(str(error))
        finally:
            worker_connection.close()

    threads = [threading.Thread(target=worker, args=(str(i),)) for i in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert not errors, errors
    assert len(outcomes) == 2, outcomes
    assert sum(accepted for _, accepted in outcomes) == 1, outcomes

    connection = sqlite3.connect(database)
    assert connection.execute('SELECT spent, reserved FROM budget').fetchone() == (0, 1)
    winner = next(intent for intent, accepted in outcomes if accepted)
    changes_before_lookup = connection.total_changes
    prior = connection.execute(
        'SELECT amount, state FROM reservation WHERE intent = ?', (winner,)
    ).fetchone()
    assert prior == (1, 'reserved')
    assert connection.total_changes == changes_before_lookup
    connection.execute(
        "UPDATE reservation SET state = 'dispatching' WHERE intent = ?", (winner,)
    )
    connection.commit()
    connection.close()

    connection = sqlite3.connect(database)
    connection.execute("UPDATE reservation SET state = 'uncertain' WHERE state = 'dispatching'")
    connection.commit()
    assert connection.execute('SELECT reserved FROM budget').fetchone() == (1,)
    assert connection.execute('SELECT state FROM reservation').fetchone() == ('uncertain',)

    connection.execute('BEGIN IMMEDIATE')
    connection.execute('INSERT INTO receipt VALUES (?, 2)', (winner,))
    connection.execute("UPDATE budget SET spent = 2, reserved = 0, state = 'breached'")
    connection.execute("UPDATE reservation SET state = 'settled' WHERE intent = ?", (winner,))
    connection.commit()
    assert connection.execute('SELECT actual FROM receipt').fetchone() == (2,)
    assert connection.execute('SELECT spent, reserved, state FROM budget').fetchone() == (2, 0, 'breached')
    denied = connection.execute('''
        UPDATE budget SET reserved = reserved + 1
        WHERE id = 1 AND state = 'open' AND spent + reserved + 1 <= ceiling
    ''').rowcount
    assert denied == 0
    connection.close()

    library_source = root / 'harness.rs'
    library_source.write_text('''pub mod policy {
    pub struct AuthorizedEffect {
        request: u64,
    }

    impl AuthorizedEffect {
        pub fn request(&self) -> u64 {
            self.request
        }
    }

    // Probe-only factory. Runtime policy and admission are separate obligations.
    pub fn scripted_authorization() -> AuthorizedEffect {
        AuthorizedEffect { request: 7 }
    }
}
''')
    consumer_source = root / 'driver.rs'
    consumer_source.write_text('''extern crate harness;
fn main() {
    let effect = harness::policy::scripted_authorization();
    assert_eq!(effect.request(), 7);
}
''')
    forge_source = root / 'forge.rs'
    forge_source.write_text('''extern crate harness;
fn main() {
    let _ = harness::policy::AuthorizedEffect { request: 9 };
}
''')

    def compile_probe(arguments):
        return subprocess.run([args.rustc, *arguments], capture_output=True, text=True)

    library = root / 'libharness.rlib'
    built = compile_probe([
        '--edition=2024', '--crate-name', 'harness', '--crate-type', 'lib',
        str(library_source), '-o', str(library),
    ])
    assert built.returncode == 0, built.stderr
    consumer = compile_probe([
        '--edition=2024', str(consumer_source), '--extern', 'harness=' + str(library),
        '-o', str(root / 'driver'),
    ])
    assert consumer.returncode == 0, consumer.stderr
    subprocess.run([str(root / 'driver')], check=True)
    blocked = compile_probe([
        '--edition=2024', str(forge_source), '--extern', 'harness=' + str(library),
        '-o', str(root / 'forge'),
    ])
    assert blocked.returncode != 0 and 'E0451' in blocked.stderr, blocked.stderr
    result = {
        'sqlite_version': sqlite3.sqlite_version,
        'rustc': compile_probe(['--version']).stdout.strip(),
        'concurrent_workers': 2,
        'budget_units': 1,
        'accepted_reservations': 1,
        'denied_reservations': 1,
        'duplicate_lookup_does_not_charge': True,
        'modeled_uncertain_retains_reservation': True,
        'overrun_receipt_and_actual_spent_retained': True,
        'breached_account_denies_admission': True,
        'external_driver_reads_approved_request': True,
        'external_token_construction_rejected': 'E0451',
        'scope': 'standalone probes, no product runtime or actual crash test',
    }
    Path(args.output).write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))

```

## Relations

- supports [harness admission design](../research/R-0004-harness-contracts.md)
- supports [storage concurrency design](../research/R-0005-storage-plugins-protocols.md)
- supports [Rust token API design](../research/R-0006-rust-implementation-contracts.md)
- addresses-design-gap-in [original proposal review](O-0001-runtime-proposal-review.md)
- required-next-by [implementation sequence](../decisions/D-0003-runtime-implementation-sequence.md)

## Revisit when

The product APIs or SQLite adapter exist. Replace feasibility-only confidence
with product-specific concurrency, compile-negative and crash-recovery evidence.
