# Service maintenance acceptance

Inventory installed and loaded services and timers before maintenance. Process
searches alone miss sleeping shell wrappers and stopped required workers.

Run `python3 scripts/ops/systemd-service-inventory.py --output /private/path/before.json`
before stopping anything. The command must exit zero. Enabled long-running
services and enabled timers must be active; idle timer-driven oneshots are normal.
The default scope is the system manager. Run it separately with `--user` as each
service-owning user for user-managed services. Keep a distinct baseline per host
and manager; a system snapshot does not cover user services or Windows tasks.
Use `--required-unit NAME` for required services that must exist independently of
the discovered installation. An already-broken baseline must be repaired first.

After restoration, run the same command with
`--baseline /private/path/before.json --output /private/path/after.json`.
Acceptance requires a zero exit code. Missing or stopped required workers fail
even if their enablement changed during maintenance. The tool is read-only and
collects no environment variables or process arguments.

For a deliberate pause, record `--paused-unit NAME` before maintenance and remove
its activation links. A pause must remain inactive after restoration and have no
incoming dependency from a required unit. Disabling enablement does not override
an explicit `Wants=` in another unit. Full cutovers must hold every dependent
worker before starting its parent; remove each hold and explicitly start that
worker only after the cutover gate passes. The complete inventory is the input
to these holds, restoration and acceptance.

The gate deliberately refuses masked/unloaded units rather than assuming their
configuration can be inspected. Resolve or explicitly inventory those holds
before capturing acceptance; never turn a failed gate into a healthy baseline.

For the owner-approved writer payout incident, keep the original unsuccessful
BEFORE intact. On the writer system manager only, supply its explicit approval
file with `--baseline BEFORE.json --approved-before-oneshot-evidence APPROVAL.json`.
The approval records the writer machine-ID SHA-256, the exact BEFORE file
SHA-256, the payout unit and its failed exit-code result. The flag accepts
only that sole timer-driven oneshot BEFORE failure. AFTER retains every original
expectation, includes the approval for audit and must report zero failures.

The TAP deposit watcher is wanted by `mayhem-stack.service` when enabled. This
restores it on stack startup after its existing `Requires=` relationship stopped
it. After installing its unit, run `systemctl daemon-reload` and
`systemctl enable mayhem-tap-deposit.service` to create the activation link.
Do not restart the stack merely to install this link.

The backup command refuses an active-stack/stopped-TAP baseline before stopping
anything. This avoids briefly activating an intentionally paused watcher while
restoring the stack. Restore the watcher or explicitly quiesce the stack before
running that backup.
