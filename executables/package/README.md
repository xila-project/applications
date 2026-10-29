# Xila `package` (native CLI)

This is a **native Xila executable**, not the host-side `xila-package`
builder. It deliberately has no graphical interface, host filesystem access,
network fetching, or launcher registration.

```text
package install /temporary/clock.xpk
package install /temporary/clock.xpk --public-key /temporary/developer.pub
package install /temporary/clock.xpk --allow-untrusted
package list
```

By default, the installer uses the raw 32-byte Ed25519 verifying key in
`/configuration/system/packages/trusted.pub`. It refuses packages when the
key is unavailable, when their publisher signature is missing/invalid, or
when any payload hash does not match. The caller can provide another
**explicitly trusted** `--public-key PATH`; this never imports a key into
system trust. `--allow-untrusted` is an **explicit temporary override**
that accepts unsigned or unknown-publisher packages, but it still
verifies their complete manifest, SHA-256 inventory, file order, TAR headers,
end marker, compatibility and sizes. An unknown signature is *not*
considered cryptographically verified under this flag.

The installer reads the archive in 512-byte blocks via the Xila VFS and
`tar-no-std` PR #73. It uses the shared incremental verification logic from
`xila-project/package_manager`, and writes to a newly created quarantine
directory. The directory is renamed into
`/applications/packages/<package-id>/` **only after** the entire archive
is verified and closed. A failed package is removed from quarantine; an
existing installed package is not deliberately replaced. Failed cleanup is
reported. Interrupted power-loss cleanup / durable transaction journal are
future work; abandoned `.incoming-*` folders are not considered installed.

No signature verification happens at launch. The OS must protect installed
package contents and manifests from unprivileged writes. This implementation
is a first step toward the package lifecycle; there is no upgrade,
uninstall, dependency solver, capability grant prompt, launcher shortcut
registration or trust-store management yet. Application entrypoints are
installed at e.g.
`/applications/packages/xila.clock/bin/clock.wasm` and can be run through
the existing Xila Wasm executable. `--allow-untrusted` must not be used
for unattended deployment.

**Important:** the package installation uses the caller's VFS permissions.
Administrators must provision writable permissions on
`/applications/packages`; there is no privilege escalation inside this CLI.

The application must be explicitly registered in each native platform's
`mount_executables!` invocation (see xila-project/platforms).
