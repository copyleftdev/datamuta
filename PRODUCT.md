# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Static HTML/CSS/JS in `site/`, three.js loaded from a CDN import map, no build step. Deployed to GitHub Pages by a workflow.

## Users

Backend and data engineers evaluating whether to try Datamuta. They arrive from the GitHub repo or a link, read technically, distrust hype, and want to know within a minute what it does, how it differs, and how far it is from production use.

## Product Purpose

Datamuta is an experimental Rust database migration runtime. It captures a typed snapshot, validates an explicit field mapping, seals data into bounded disk partitions, publishes an exactly approved plan, resumes from native commit evidence, and verifies stored destination values. Success for the site: an engineer understands the mechanism, trusts the honesty of the scope, and clones the repo to run the PostgreSQL example.

## Positioning

Evidence over progress bars. The plan is approved by exact SHA-256 hash; each partition's records, receipt and cursor commit atomically in the destination; resume reads native commit evidence; verification compares stored values against the sealed, Merkle-authenticated spool. It refuses unsupported conversions rather than guessing.

## Operating Context

CLI (`datamuta-stream`): configure → capture → inspect → run (with `--approved-plan-sha256`) → status → verify. Local Docker Compose example with two loopback PostgreSQL 17 containers and three synthetic rows. Source can be offline after capture.

## Capabilities and Constraints

- PostgreSQL: source and target; complete documented example; still experimental.
- MySQL (target), MongoDB (target): laboratory loopback profiles. Firestore (source and target): local emulator only.
- Mapping: bool, i32/i64, UTF-8, null; copies/renames; lossless i32→i64 widening; integer identities. Inserts into prepared targets only.
- Limits: record ≤ 64 KiB, partition ≤ 256 rows / 1 MiB, spool 8 KiB–4 GiB (default 256 MiB), ≤ 65,536 partitions, 300 s per command.
- Not included: CDC, cutover, rollback, decimals/timestamps/arrays, nested transforms, petabyte qualification, hosted service.

## Brand Commitments

Name "Datamuta". Always labelled experimental and not production-ready. Visual reference pinned by the user: isometric line-art system diagram in the manner of asteroid.ai's hero (inspiration only; no copying of their mark or content).

## Evidence on Hand

README, mdBook in `docs/book/`, `examples/postgres/` (synthetic data). No users, benchmarks, testimonials, logos or throughput numbers exist; never invent them.

## Product Principles

1. State scope exactly; experimental status is visible, never buried.
2. Show the mechanism, not adjectives.
3. Every claim traces to the README or the code.
4. Point to the runnable example as the next step.
