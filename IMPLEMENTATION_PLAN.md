# Public-beta implementation plan

Work is local on a topic branch. Publishing, tags, remote settings, and public
issue creation require explicit authorization. Existing OSS infrastructure is
preserved. Each stage must retain passing tests before distribution.

1. **P0 transport isolation:** reproduce wrong-identity and suffix collision;
   bind endpoints and handshakes to validated stores; reject stale endpoints;
   test concurrent clients, replaced files, direct and daemon access.
2. **P1 installation:** retain default local embedding capability while testing
   a lexical build without ONNX; measure dependencies/artifacts; add macOS CI
   without changing required check names.
3. **P1 retrieval:** commit synthetic evidence and assertions for relevant,
   missing, scoped, retired and contradictory records; run a real model test;
   pin and verify downloaded bytes. Report limitations, not benchmark scores.
4. **P1 maintenance:** expose online snapshot, integrity verification and
   restore-to-new-file; preserve identity, refuse overwrite/migration, test
   populated round trips, bad files and running daemons.
5. **P1 distribution:** prepare versioned source/binary candidate validation,
   checksums and provenance; document reviewed-commit release/upgrade process.
6. **P2 contributors:** align purpose and commands, record tested client scope,
   propose actionable public issue drafts locally, resolve private conduct
   contact without inventing an address.
7. **Acceptance:** run fresh-source install, demo, retrieval, backup/restore,
   CI-equivalent checks and candidate install. Record environment, exact commands,
   outcomes and unresolved gates. No release if a critical defect remains.
