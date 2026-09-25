# Windows code signing

The release workflow signs the application executable, MSI and NSIS installer with
Authenticode. Branch and manual workflow checks may build unsigned; a `v*` tag fails before
the build if signing is not configured.

## Certificate requirements

- A currently valid, trusted RSA code-signing certificate.
- The certificate and private key exported as a password-protected PFX.
- The certificate must include the Code Signing enhanced key usage
  (`1.3.6.1.5.5.7.3.3`).

Microsoft Artifact Signing is preferable for a new public release because the private key
does not need to be exported. The current workflow implements the portable-PFX path; adopting
Artifact Signing requires replacing `prepare-signing.ps1` with its Tauri `signCommand`.

## Configure the GitHub secrets

Never commit the PFX or its password. Add these repository Actions secrets:

| Secret | Value |
|---|---|
| `WINDOWS_CERTIFICATE` | Base64-encoded bytes of the PFX |
| `WINDOWS_CERTIFICATE_PASSWORD` | PFX export password |

Using GitHub CLI from PowerShell:

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes('certificate.pfx')) |
  gh secret set WINDOWS_CERTIFICATE
gh secret set WINDOWS_CERTIFICATE_PASSWORD
```

The second command prompts for the password without putting it in the command line.

## Verify before tagging

Run the Release workflow manually from a branch. If the secrets are present, the build imports
the PFX, validates its private key, expiry and Code Signing usage, and lets Tauri sign before it
creates the installers. `verify-signatures.ps1` then requires valid signatures from that exact
certificate on all three artifacts.

Only create a version tag after this signed branch run succeeds. The release workflow refuses
to build a version tag when either secret is missing.
