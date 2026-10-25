# QR login verification

Date: 2026-10-25.

X: login command reused and verified a real existing Chrome session. All139 workspace tests pass; cross-device Passkey QR availability requires enrolled phone and browser/OS support. No native App QR endpoint claimed.

No phone-confirmation login was performed during automated verification. Existing sessions were preserved. Unit/regression tests and challenge generation are separate evidence from complete live authentication.

Regression result: 142 tests passed in the complete working checkout.

Independent review fixes: empty/null/unavailable Viewer results are rejected; Linux browser launch is detached so polling/timeout can proceed. Both regressions failed first and passed after the fixes. Cookie domains use exact root/subdomain matching.
