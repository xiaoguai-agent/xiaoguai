# GDPR DPIA template — Xiaoguai

A Data Protection Impact Assessment (DPIA) is required under Article 35
of the GDPR whenever a processing activity is "likely to result in a
high risk to the rights and freedoms of natural persons". Hosting an AI
agent platform that ingests user messages typically triggers this.

This template walks through the seven sections every DPIA must cover.
Replace bracketed text with your deployment-specific answers before
submission.

## 1. Description of processing

- **Controller:** [Your legal entity]
- **Processors:** Xiaoguai operator (you), the LLM provider(s) you have
  registered, MCP servers (each is a separate processor if it forwards
  data outside your boundary).
- **Categories of data subjects:** Authenticated users of the platform
  + anyone whose data appears inside a chat message or tool result.
- **Categories of personal data:**
  - Identifiers (user_id, tenant_id, IP captured by ingress logs).
  - Free-text message content (may contain any of the special
    categories of Art. 9 depending on user behaviour).
  - Token usage (`token_usage` table — billing-relevant).
- **Lawful basis (Art. 6):** [Contract / Legitimate interest /
  Consent — pick per use case].
- **Retention period:** sessions/messages retained until tenant deletion
  request or [N] days of inactivity (configure via cron in your
  operator workflows; Xiaoguai itself doesn't auto-delete).

## 2. Necessity and proportionality

- The processing is **necessary** because [explain why the AI agent
  function cannot be provided without storing the conversation history].
- **Proportionality** is achieved by:
  - Storing only what's needed for replay (no raw HTTP request bodies).
  - Per-tenant RLS — one tenant cannot read another's messages.
  - LLM provider calls are made under the tenant's own quota and
    contractual relationship with that provider (no implicit data
    sharing across tenants).

## 3. Risk identification

| Risk                                         | Likelihood | Severity | Mitigation                                   |
|----------------------------------------------|:----------:|:--------:|----------------------------------------------|
| Unauthorised access to messages              | Low        | High     | OIDC + RBAC + RLS, audited.                  |
| Data exfiltration via MCP tool               | Medium     | High     | Per-tenant MCP allowlist; default-deny netpolicy in v1.1. |
| LLM provider retains prompts beyond your contract | Medium | Medium  | Use providers with zero-retention agreements; document in the registered provider's `terms` field. |
| Audit-log tampering                          | Low        | High     | HMAC chain (`xiaoguai-audit`).                |
| Backup leak                                  | Low        | High     | Encrypt backups at rest; restrict the key.    |

## 4. Measures to address risks

(Cross-reference your Helm values + secrets configuration here.)

- Transport: TLS 1.2+ enforced at ingress.
- At rest: Postgres + Valkey backups encrypted; RDS key managed by you.
- Access control: OIDC + Casbin RBAC; quarterly access reviews.
- Logging: every write action emits an audited HMAC-chained entry.
- DSR (Subject Access / Erasure): operator uses
  `xiaoguai mcp / provider / session admin` paths plus PG-level DELETE
  scoped by `tenant_id` to fulfil requests within Art. 12 timelines.

## 5. Cross-border transfers (Chapter V)

Document each LLM provider you've registered:

| Provider          | Data centres      | Transfer mechanism                                 |
|-------------------|-------------------|-----------------------------------------------------|
| Ollama (local)    | On-prem           | None — no transfer.                                 |
| DeepSeek / 智谱   | PRC               | If data subjects are in the EU: SCCs + TIA needed.  |
| OpenAI            | US                | EU-US Data Privacy Framework.                       |

## 6. Consultation

- DPO sign-off: [name + date]
- Affected data subjects: consult via [route — privacy notice update,
  user representative panel, etc.]

## 7. Decision

- [ ] Approved as documented.
- [ ] Approved with the following additional measures: [...]
- [ ] Rejected; processing will not commence.

This template is **not legal advice**. Engage your DPO and counsel.
