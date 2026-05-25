# Contract Review Primer — Key Clauses and Red Flags

Source: Original work released under MIT License by xiaoguai contributors
License: MIT
Corpus role: Contract review checklist, red-flag clause patterns, negotiation guidance

---

## Purpose

This primer identifies the highest-risk clauses in common commercial agreements and provides
guidance for legal review, risk flagging, and negotiation.

## 1. Indemnification

### What to look for
Indemnification clauses determine who pays when a third party makes a claim. They are among the
highest-risk clauses in commercial agreements.

### Red Flags
- **One-sided indemnification**: Only one party indemnifies; the other bears no obligation.
- **Uncapped indemnification**: Indemnification obligation has no dollar cap. Combined with
  aggressive scope ("any and all claims") this can create unlimited exposure.
- **Consequential damages included**: Indemnification explicitly covers lost profits, business
  interruption, or punitive damages. Push to exclude.
- **"Claims arising from your use"**: Overly broad trigger — covers even claims the other party's
  product caused, if the product was in use.

### Preferred Language
- Mutual, reciprocal indemnification obligations.
- Scope limited to direct, out-of-pocket damages (not consequential).
- Cap tied to fees paid in the prior 12 months.
- Carve-out for indemnitee's own negligence or willful misconduct.

## 2. Limitation of Liability

### Red Flags
- **Asymmetric cap**: Vendor's liability capped at 1 month of fees; customer's liability uncapped.
- **Intentional exclusion of cap for IP claims**: Common vendor tactic — unlimited exposure
  for customer if they inadvertently infringe a third-party IP right.
- **No cap exception for data breach**: Some agreements exclude data breach liability from
  limitation; others don't — risk flag if you are processing sensitive data.
- **Mutual exclusion of consequential damages**: Reasonable, but watch for carve-outs that
  gut the limitation (e.g., fraud, confidentiality breach excluded from cap).

### Benchmark Caps
- SaaS subscription: 12 months of fees paid
- Professional services: total fees paid under the SOW
- Enterprise MSA: negotiated; often 12-24 months of total fees

## 3. Intellectual Property Ownership

### Red Flags
- **No work-product assignment**: Vendor retains all IP; customer gets only a license. Problematic
  for custom-built software or tailored deliverables.
- **Broad Background IP license**: Vendor's background IP license terminates on contract end,
  leaving the customer unable to maintain the delivered work.
- **Open-source contamination**: Vendor uses GPL-licensed components in deliverables without
  disclosure; customer's proprietary product may be at risk.
- **"Feedback" IP trap**: Any feedback or suggestions about the product irrevocably assigned to
  vendor, including customer's own engineering insights.

## 4. Termination and Renewal

### Red Flags
- **Auto-renewal with short opt-out window**: Annual auto-renewal requires cancellation notice
  90+ days in advance — easy to miss.
- **Termination for convenience notice period too long**: 180-day notice period for termination
  for convenience locks customer in despite problems.
- **No termination for cause**: No right to terminate immediately if the other party commits
  a material breach.
- **"Termination for convenience" triggers early-termination fee**: Effectively penalizes the
  customer for exercising contractual rights.

### Data and Transition
- What happens to customer data on termination?
- Is there a transition period (e.g., 60-90 days continued access)?
- Is there a data export obligation?

## 5. Representations and Warranties

### Red Flags
- **Disclaimer of all warranties**: "As-is" disclaimer removes all warranty protection.
  Acceptable for experimental or beta software; not for production-grade commercial services.
- **No warranty of non-infringement**: Exposes customer to third-party IP claims without
  recourse against the vendor.
- **Accuracy warranty missing for data products**: If the product provides data (pricing,
  analytics, medical information), absence of accuracy warranty is a red flag.

## 6. Governing Law and Dispute Resolution

### Red Flags
- **Foreign governing law**: Agreement governed by law of a foreign jurisdiction — higher
  litigation cost and unpredictable legal standards.
- **Mandatory arbitration with no injunctive relief carve-out**: Prevents customer from
  seeking emergency injunctive relief in court.
- **Class-action waiver**: Eliminates class-action rights in consumer or employment contexts
  (enforceability varies by jurisdiction).
- **One-sided venue**: Dispute must be resolved only in vendor's home jurisdiction.

## 7. Change-of-Control Provisions

### Red Flags
- **No assignment restriction on vendor**: Vendor can assign the contract (including customer
  data obligations) to a competitor without customer consent.
- **No change-of-control termination right for customer**: If vendor is acquired, customer
  cannot exit. Critical in M&A scenarios.

## 8. Quick Red Flag Checklist

- [ ] Uncapped indemnification on any side
- [ ] Liability cap below 6 months of fees
- [ ] No work-product IP assignment for custom deliverables
- [ ] Auto-renewal with opt-out window > 60 days
- [ ] Governing law: foreign jurisdiction
- [ ] No data return/deletion obligation on termination
- [ ] No security / breach notification obligation
- [ ] No SLA with defined remedy
- [ ] "As-is" warranty for production service
- [ ] No termination for cause right
