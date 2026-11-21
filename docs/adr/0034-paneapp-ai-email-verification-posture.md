# ADR-0034: paneapp.ai Email Verification Posture — Disabled, Stripe-as-Identity

**Status:** Accepted  
**Date:** 2026-05-10  
**Author:** Matt Carano, Claude (spec session)  
**Supersedes:** None  
**Supplements:** ADR-0031 (paneapp.ai Signup Flow), ADR-0028 (Ecom Platform Selection)  
**Related:** ADR-0027 (Pane↔Ecom API Contract), ADR-0029 (Entitlement Response Shape and Email-Check Privacy Posture)

## Context

ADR-0031 §Decision orders the signup flow as Option A (account creation before Stripe Checkout). ADR-0031 §Negative explicitly defers one design call to a later ADR:

> **Email verification timing.** Supabase Auth defaults to requiring email verification before sign-in. paneapp.ai must either disable email verification (acceptable trade for paid customers, given Stripe verifies them via payment) or surface the verification step explicitly. This is a paneapp.ai-side decision with implications for the customer experience between payment and first sign-in.

This ADR resolves that call.

The two viable postures:

**Posture A — Email verification enabled.** Supabase Auth sends a verification email on signup. The customer must click the link before they can sign in to Pane (Supabase Auth blocks sign-in attempts on `email_confirmed_at IS NULL`). paneapp.ai must surface this step explicitly between payment and Pane install.

**Posture B — Email verification disabled.** Supabase Auth's "Confirm email" project setting is off. New users are created with `email_confirmed_at` populated immediately on signup. The customer goes from payment to Pane sign-in with no intermediate verification step.

The two postures produce structurally different post-payment customer experiences and require different recovery surfaces.

### What if we didn't decide

ADR-0031 already named this as a paneapp.ai-side decision. Without an ADR:

- **The default ships.** Supabase Auth defaults to verification enabled. Without an explicit decision, paneapp.ai inherits the default — and Posture A's friction (verify email between payment and install) lands on the customer at exactly the moment ADR-0031 §Rationale optimized away from.
- **Recovery paths get invented at build time.** Posture A's failure modes (typo'd email, customer doesn't see the verification email, customer clicks a stale link) need surfaces. Without a decided posture, those surfaces get bolted on under deadline pressure.
- **Future revisits have no documented baseline.** Post-launch abuse signals (synthetic accounts, throwaway emails, etc.) will produce a question of "do we turn verification back on?" Without an ADR pinning the v1 reasoning, that conversation has nothing to push against.

## Decision

**Email verification is disabled in v1.** Supabase Auth project setting "Confirm email" is set to off. New users are created with `email_confirmed_at` set at signup-time, allowing immediate sign-in to Pane after payment.

### Implementation

| Layer | Setting |
|---|---|
| Supabase Auth project | "Confirm email" → off |
| `auth.users` row at signup | `email_confirmed_at` populated by Supabase Auth automatically |
| Pane sign-in (S1a → S1b) | No verification check; ADR-0029 email-check returns `{exists: bool}` only |

The wrapper's signup endpoint (`POST /v1/auth/signup`, per ADR-0032 §3 service role boundary) calls Supabase Auth's admin user-create with `email_confirm: true` to seed `email_confirmed_at` directly, ensuring the customer is sign-in-ready the moment the wrapper returns success.

### Recovery surfaces (covered by other decisions)

- **Typo'd email.** Customer paid with a typo'd email; cannot sign in to Pane (S1c "no account found"). Recovery: customer contacts support; support cancels the Stripe subscription and refunds; customer signs up again. v1 has no self-service email-change.
- **Customer forgets password.** Standard Supabase Auth password reset flow; ADR-0027 §Forgot password link from Pane S1b routes to ecom reset. paneapp.ai surfaces a `/forgot-password` page that calls `auth.resetPasswordForEmail()`.
- **Customer never installs Pane.** Stripe billing email (separate from Supabase Auth email) is captured at Checkout. The customer can manage their subscription via Stripe Customer Portal even if they never sign in to Pane. Cancellation and refund paths are intact.

## Rationale

### Why disabled, specifically

**Stripe payment is sufficient identity proof for v1.** The customer provided a credit card, completed Stripe's fraud checks, and paid $29. The signal that "this is a real person at this email" is stronger than "this person clicked a link in an email." Supabase Auth's verification flow is designed for free-tier products where payment isn't an identity primitive; for a paid SaaS at signup, it's redundant.

**Friction at the worst possible moment.** ADR-0031 §Rationale optimizes the post-payment-to-first-use path. A verification step between payment and Pane sign-in adds:

- A second tab to monitor (the email)
- A delivery delay (verification email round-trip; spam-folder risk)
- A second decision point ("did the link work?" "is this email from Pane real?" "wait, do I need to wait for it to arrive?")

The customer has already decided. The product should reward the decision with immediate access, not test it again.

**Email-check oracle posture (ADR-0029) is preserved.** Disabling verification doesn't change the email-check privacy posture — the wrapper still returns `{exists: bool}` only, rate-limited per IP. The verification toggle is independent of the existence-oracle question.

**The "what if email is wrong" failure mode is bounded.** Typo'd email at signup is the worst case. v1 handles it via support intervention (refund + re-signup). This is acceptable at launch traffic (~100 customers week one) where support is direct. If the rate of typo'd emails proves meaningful at scale, post-launch revisit can introduce verification (Posture A) or a separate email-change flow.

### Why not Posture A at v1

Supabase Auth's verification flow is a known quantity, not technically risky. The case against it is purely UX:

- Adds a step at the moment of highest customer commitment
- Email deliverability is a real failure mode (Pane's domain is new; some email providers will route initial sends to spam)
- Most customers expect immediate access after payment in industry-standard SaaS UX

If post-launch signals indicate verification is needed (synthetic-account abuse, support load from typo'd emails, regulatory pressure), Posture A is reachable via a single Supabase project setting change. The decision is reversible.

### Why this is ADR-worthy and not just a spec note

- **It changes the security/identity posture of the product.** "Who is verified" is a load-bearing question for trust, support, and compliance. Pinning the v1 stance lets future-us reason about why we made it.
- **ADR-0031 §Negative explicitly named it as deferred.** The deferral was an honest "this is a paneapp.ai-side call we'll resolve in the paneapp.ai ADR." Closing the loop with an ADR is the discipline match.
- **Reversibility is bounded but real.** Switching to Posture A post-launch affects existing customers (who got in without verification — do we retroactively prompt them?). The migration path matters; documenting the v1 baseline lets a future migration plan reason against it.

## Consequences

### Positive

- **Frictionless post-payment flow.** Customer pays, lands on `/subscribe/success`, downloads Pane, signs in. No verification email round-trip.
- **No deliverability risk during signup.** A new domain's email reputation isn't tested at the most critical customer moment.
- **Simpler support story.** Support doesn't field "I didn't get the verification email" tickets in launch week.
- **Pane S1a/S1b/S1c flow doesn't change.** ADR-0029's email-check oracle posture is unaffected; sign-in routing logic stays as specified.

### Negative

- **Typo'd-email customers can't self-recover.** A customer who pays with `admn@paneapp.ai` instead of `admin@paneapp.ai` cannot sign in to Pane. Support intervention required (cancel + refund + re-signup). At launch volume this is acceptable; at scale it becomes a real cost.
- **Synthetic-account abuse is bounded only by Stripe.** Without verification, an attacker could create accounts at any email (typo'd, throwaway, someone else's). Stripe's fraud checks bound this — payments still require valid cards — but the abuse vector is wider than Posture A.
- **Future migration to Posture A is non-trivial.** Existing customers were created with `email_confirmed_at` populated. A future enforcement of verification needs a "prompt existing customers to verify" flow that doesn't lock them out of Pane during the prompt period. Migration plan is post-launch concern; documenting v1 baseline now makes future planning easier.

### Risks

- **A high typo rate at signup creates support load.** Mitigation: client-side email-confirmation field on the signup form (customer types email twice; we compare). Trivially cheap; large UX polish for the typo case. Lands as a content slot in the paneapp.ai design brief — promoted from "recommended" to expected-deliverable at design hand-off.
- **A regulatory or compliance requirement surfaces post-launch** (e.g., a B2B customer demands verified-email proof before paying). Mitigation: Posture A is reachable via Supabase setting toggle + a paneapp.ai page surfacing the verification step. Reversible.
- **A future Tier 1 hosted plan introduces team accounts where email verification matters more** (members of the team need verified identities for permission checks). Out of scope for v1; Tier 1 hosted reopens the question.

## Alternatives Considered

**Posture A — Email verification enabled.** The Supabase default. Rejected for v1 on UX grounds in §Rationale. Reachable post-launch via a single setting change.

**Hybrid — Enabled but soft-blocked.** Verification email is sent, but Pane sign-in succeeds without it; a banner in Pane prompts verification asynchronously. Rejected because (a) it requires Pane-side UI changes (a banner state in onboarding §6.1 that doesn't currently exist), (b) it asks the customer to do something they didn't expect to do, and (c) the security benefit is unclear if sign-in works without verification anyway.

**Verify via SMS instead of email.** Higher-friction primitive, additional vendor (Twilio or similar), real cost per send. Rejected as solving the wrong problem — the v1 question is whether to verify at all, not which channel to use.

**Use Stripe's verified email as the Pane account email automatically.** Considered: paneapp.ai signup form takes only password; account email is the email Stripe captures during Checkout (which Stripe verifies via payment-method on file). Rejected because (a) it inverts ADR-0031's order (account would be created post-Checkout, falling into rejected Option B territory), and (b) Stripe's email field is the billing email, which the customer may want different from their Pane account email. The two emails serve different purposes; conflating them removes flexibility.

**Defer the decision; ship Posture A and revisit if friction shows.** Rejected because Posture A's friction is the launch-day customer experience. Optimizing it post-launch means the launch cohort got the worse UX. Decisions that affect launch UX should land before launch.

## References

- ADR-0031 — paneapp.ai Signup Flow (deferred this decision in §Negative)
- ADR-0027 — Pane↔Ecom API Contract (sign-in flow assumes account-exists-on-arrival)
- ADR-0029 — Entitlement Response Shape and Email-Check Privacy Posture (oracle posture preserved)
- ADR-0028 — Ecom Platform Selection (Supabase Auth + Stripe + thin wrapper)
- ADR-0032 — paneapp.ai Stack and Hosting (signup endpoint contract)
- Site spec: `docs/spec/paneapp-ai.md` §2 (signup flow specifics)
- Pane spec: `docs/spec/onboarding.md` §6.1 (sign-in flow)
- Supabase Auth confirm-email docs: https://supabase.com/docs/guides/auth/auth-email
- Sub-tasks named by this ADR:
  1. Set Supabase Auth project setting "Confirm email" to off pre-launch
  2. Add `email_confirm: true` flag to wrapper's `POST /v1/auth/signup` admin call (when that endpoint contract is designed)
  3. Add client-side email-confirmation field (type-twice) to paneapp.ai signup form to mitigate typo failure mode
  4. Add `/forgot-password` page to paneapp.ai for password-reset recovery
  5. Post-launch: monitor support ticket rate for typo'd-email cases; revisit posture if rate exceeds threshold
