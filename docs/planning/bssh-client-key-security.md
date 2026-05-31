# bssh Client Key Security

## Current state

`bssh --identity <path>` uses a caller-supplied RSA identity for SSH
user-authentication. The older `bssh --publickey` path still uses the
compiled-in Breenix RSA keypair as a development/test fallback.

The key flow is:

- `userspace/programs/src/bssh.rs` selects `ClientAuthMethod::PublicKey`.
- `libs/libbreenix/src/ssh/transport.rs` calls `auth::client_auth_publickey`.
- `libs/libbreenix/src/ssh/auth.rs` gets the public key from
  `keys::embedded_client_public_key_blob()` and signs with
  `keys::sign_with_embedded_client_key()`.
- `libs/libbreenix/src/ssh/keys.rs` implements both helpers by loading
  `HostKey::load()`, whose private exponent is compiled from `HOST_KEY_D`.

That fallback identity is not a private user identity. It is repository key
material and must not be authorized for login to real external accounts.

## Required production fix

Use a caller-supplied client identity for real hosts:

```text
bssh user@host --identity /path/to/id_rsa
```

The first implemented identity format is unencrypted PKCS#1 PEM RSA. Generate
compatible proof keys with:

```text
ssh-keygen -t rsa -b 3072 -m PEM -f id_rsa -N ''
```

Future work can add OpenSSH private-key format parsing and encrypted-key
passphrases. Real-host publickey proofs must not rely on adding the embedded
Breenix key to a real account's `authorized_keys`.
