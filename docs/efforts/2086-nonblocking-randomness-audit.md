# Boot-time random readers (S42)

Audit of libc, libbreenix, the pinned Rust standard library, and init after #2084.
The flag chooses output from the existing pool before credited entropy readiness;
it does not credit entropy or substitute a time/PID generator.

| Reader | Classification and policy |
| --- | --- |
| libbreenix DNS `generate_txid` | Transaction identifier, `GRND_INSECURE`. |
| libc `make_unique` | Temporary names for mkstemp, mkstemps, mkostemp, mkostemps and mkdtemp, `GRND_INSECURE`; exclusive creation and permissions stay unchanged. |
| libc stdio `temp_name` | Names for tmpnam and tmpfile, `GRND_INSECURE`; collision checks and tmpfile exclusive creation stay unchanged. |
| libbreenix temporary-file equivalents | No additional name generator found; Rust users can reach libc or create their own files. |
| Rust std HashMap/RandomState seeds | Already use `GRND_INSECURE` in the pinned fork, commit 666252fd, `std/src/sys/random/breenix.rs`; no fork change needed. |
| Rust std `DefaultRandomSource` / `fill_bytes` | General-purpose secure random API, retains `flags = 0`. No init call found apart from the separate HashMap path. |
| libc getrandom and syscall forwarding | Preserve the caller's flags; do not globally downgrade a secure API. |
| libbreenix `getrandom_bytes` / `Csprng::new` | Secure API, retains `flags = 0`. |
| crypto suite CSPRNG seeds | Exercise secure `Csprng::new` and default `getrandom_bytes`; retain blocking reads. |
| hello_std_real random bytes | Required secure-default boot check, `flags = 0`; retain blocking reads. |
| console getty password/salt | Already disposable fixtures using `GRND_INSECURE` under S41. |
| SSH transport, packet RNG and key exchange | Session-key material and protocol randomness, keep secure `Csprng::new`. |
| TLS handshake | Session-key material, keep secure `Csprng::new`; no automatic init TLS caller found. |
| SSH host-key load | Loads pre-generated material, not a runtime random read. |
| Persistent passwords | No boot-time random password generator found in the audited init and libraries. |
| libc rand/random families | Explicitly seeded deterministic generators, not pool readers. |
| getrandom_unready guest probe | Deliberately exercises blocking and interruption with a bounded parent; not an automatic boot consumer. |

## Secure consumer reached at boot

[Issue #2086](https://github.com/ryanbreen/breenix/issues/2086) records the default
init path: `start_bsshd` followed by synchronous `run_bssh_pubkey_oracle` creates
SSH session keys while Parallels and VMware can still have an unready pool.
The optional `BREENIX_BSSH_AUTORUN` path also starts SSH handshakes. These reads
must remain secure; defer the handshakes until credited entropy is available.
The listening daemon alone waits for a connection before constructing its
session. Suite mode bypasses init, so all-suite evidence does not cover this
separate default-init defect. S42 requests this consumer be reported, rather
than weakening its secret reads.
