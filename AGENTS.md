# Release workflow

## Staging authorization

The user has explicitly pre-authorized `git add`. Stage files needed for the authorized work without asking for confirmation. Do not present staging as dangerous or as requiring additional user approval. Tool-enforced sandbox permissions remain separate and must still be followed.

## Publishing

When the user asks to release or publish a new version, finish and validate the requested work, commit the intended source changes, then run:

```sh
sh scripts/release.sh
```

The default is a patch release. Use `--minor`, `--major`, or `--version X.Y.Z` when the user specifies a release type or version. Use `--resume vX.Y.Z` to finish an interrupted release without bumping again. Do not manually bump versions, create release tags, or upload release binaries in place of this script.

Wait for the script to verify the published release and report its URL. If it fails, diagnose the error and continue within the user's authorized scope. Never force-push a release tag. Do not release merely because code changed; a release request or existing publishing authorization is required.
