# bssh Client Key Security

## Current state

`bssh --publickey` currently uses the compiled-in Breenix RSA keypair as its
SSH user-authentication identity.

The key flow is:

- `userspace/programs/src/bssh.rs` selects `ClientAuthMethod::PublicKey`.
- `libs/libbreenix/src/ssh/transport.rs` calls `auth::client_auth_publickey`.
- `libs/libbreenix/src/ssh/auth.rs` gets the public key from
  `keys::embedded_client_public_key_blob()` and signs with
  `keys::sign_with_embedded_client_key()`.
- `libs/libbreenix/src/ssh/keys.rs` implements both helpers by loading
  `HostKey::load()`, whose private exponent is compiled from `HOST_KEY_D`.

That means the current publickey identity is not a private user identity. It is
repository key material and must not be authorized for login to real external
accounts.

## Required production fix

`bssh` should support a caller-supplied client identity, for example:

```text
bssh user@host --identity /path/to/id_rsa --publickey
```

The client should parse an OpenSSH-compatible private key or a constrained
Breenix key format from the filesystem, derive the matching public key blob,
and sign USERAUTH data with that private key. The embedded repo key can remain
only as a local development/test identity, and `bssh` should label it as such in
the CLI.

Until that exists, real-host publickey proofs must not rely on adding the
embedded Breenix key to a real account's `authorized_keys`.
