# Organization transfer checklist

This checklist deliberately leaves the destination organization and team handles unspecified. Fill
them in during transfer approval; do not guess ownership, URLs, or licensing decisions.

## Prepare the local repository

- Review `git status --short`, `git diff`, and untracked files. Commit the agreed transfer set on a
  reviewed branch so the repository is not transferred with ambiguous local-only work.
- Require the current CI and Considered workflows to pass, including secret-history/tree scans,
  dependency audit, formatting, Clippy, workspace tests, Postgres migration conformance,
  deterministic semantic benchmark, production configuration validation, demo checks, and
  macOS/Windows portability checks.
- Run `considered check` locally and keep every variance at or below its measured value.
- Review Git history and the transfer tree for credentials, personal data, local paths, generated
  databases, audit logs, and proprietary fixtures.
- Decide whether historical author email addresses may be retained. Rewriting them changes commit
  IDs and requires a separately approved migration and coordination plan.
- Confirm the license policy with the team and counsel. The workspace currently declares MIT; do
  not change or remove it as part of a mechanical transfer.
- Review RustSec findings. Accept an exception only with an owner, rationale, mitigation, expiry
  date, and tracking issue; time-box and remove exceptions when the dependency is fixed.

## Supply destination-specific metadata

- Replace the existing root `CODEOWNERS` personal handle with real destination-team handles (or
  move the file to `.github/CODEOWNERS`) and require their review on sensitive paths. Keep the
  current file unchanged until those handles are known.
- Replace the workspace `Cargo.toml` `repository` URL after the final organization/repository name is
  approved. Check package metadata, badges, links, release automation, and external documentation
  for the old location.
- Inventory Actions secrets, environments, deploy keys, webhooks, installed apps, branch rules,
  package permissions, and external integrations. Recreate only what is needed with least privilege.

## Transfer and update clones

1. Transfer the repository using the hosting provider's organization-transfer flow and verify the
   destination administrators can manage it.
2. Update each trusted clone after the destination URL is confirmed:

   ```sh
   git remote set-url origin git@github.com:<organization>/<repository>.git
   git remote -v
   git fetch --prune origin
   ```

3. Verify Actions, release/package permissions, redirects, webhooks, and protected environments in
   the destination account before merging or deploying.

## Protect the destination

- Protect the default branch and require pull requests, review, resolved conversations, and these
  exact status checks: `CI / Rust`, `CI / Portability (macos-latest)`,
  `CI / Portability (windows-latest)`, `CI / Demo`, and `Considered / check`. Confirm the provider's
  displayed context names before saving the rule.
- Enable secret scanning, push protection, Dependabot security updates, and CodeQL/default code
  scanning. Keep Actions permissions read-only by default and pin third-party actions.
- Enable private vulnerability reporting and update [SECURITY.md](../SECURITY.md) with the approved
  destination security channel.
- Provision a dedicated database or schema and dedicated credentials for each tenant; never use one
  database trust boundary for unrelated tenants. Run migrations, backup/restore validation, and
  production preflight independently for each deployment.
- Configure and monitor automatic WORM export/anchoring of audit-log heads outside the service trust
  boundary, and complete a restore and credential-rotation drill before production traffic.

## Close the transfer

- Open a small reviewed pull request for the approved CODEOWNERS, Cargo repository URL, security
  contact, and any badges or links that could not be finalized earlier.
- Confirm required checks pass under destination branch protection and that a test pull request
  cannot merge without them.
- Record owners for dependency updates, RustSec exceptions, backups, audit anchors, incident
  response, and periodic access review.
