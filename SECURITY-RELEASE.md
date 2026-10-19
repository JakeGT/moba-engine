# Coordinated security release checklist

Private workspace: `JakeGT/moba-engine-security`.

## Before merge to public

- [ ] Merge current public `main` into private `main`; resolve conflicts and re-run full tests.
- [ ] Bump `PROTOCOL_VERSION` for the sealed-payload counter prefix, ContentUpdate retry/phase behavior and attestation v2.
- [ ] Bump workspace version to the coordinated release version and document content/attestation compatibility.
- [ ] Revalidate/re-sign all shipped content packs with attestation v2.
- [ ] Run `scripts/agent-verify.sh --full` on the merged private tree.
- [ ] Obtain independent security review of the private diff.

## Publish

- [ ] Push the reviewed tree as a public branch and open the release PR (squash only).
- [ ] Wait for all required checks, including renderer/release/MSRV.
- [ ] Squash-merge, tag the patched release, and verify release artifacts.
- [ ] Publish all four advisories with patched version and credit the reporter.
- [ ] Close the private advisory records and archive/remove the private workspace only when no follow-up is needed.

## Do not

- Do not include exploit details in public PRs/notes.
- Do not publish advisories before the patched artifacts exist.
- Do not merge the private tree without re-running the full gate on the merged public base.
