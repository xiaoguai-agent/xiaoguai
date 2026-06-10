# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: admin-ui/admin-personas.spec.ts >> admin-ui Personas pane — CRUD against mocked /v1/personas >> "New persona" drawer opens, submits, list refreshes
- Location: tests/admin-ui/admin-personas.spec.ts:183:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/personas
Call log:
  - navigating to "http://localhost:5174/personas", waiting until "load"

```

# Test source

```ts
  85  |         name: body.name,
  86  |         system_prompt: body.system_prompt ?? '',
  87  |         default_model: body.default_model ?? null,
  88  |         tool_allowlist: body.tool_allowlist ?? null,
  89  |         escalation_tier: body.escalation_tier ?? null,
  90  |       });
  91  |       store.push(created);
  92  |       await route.fulfill({
  93  |         status: 201,
  94  |         contentType: 'application/json',
  95  |         body: JSON.stringify(created),
  96  |       });
  97  |       return;
  98  |     }
  99  | 
  100 |     // PATCH /v1/personas/:id (update)
  101 |     const patchMatch = /\/v1\/personas\/([^/?]+)$/.exec(url.pathname);
  102 |     if (method === 'PATCH' && patchMatch) {
  103 |       const id = patchMatch[1];
  104 |       const body = JSON.parse(req.postData() ?? '{}');
  105 |       const idx = store.findIndex((p) => p.id === id);
  106 |       if (idx === -1) {
  107 |         await route.fulfill({ status: 404, body: '{}' });
  108 |         return;
  109 |       }
  110 |       store[idx] = { ...store[idx], ...body };
  111 |       await route.fulfill({
  112 |         status: 200,
  113 |         contentType: 'application/json',
  114 |         body: JSON.stringify(store[idx]),
  115 |       });
  116 |       return;
  117 |     }
  118 | 
  119 |     // DELETE /v1/personas/:id
  120 |     const delMatch = /\/v1\/personas\/([^/?]+)$/.exec(url.pathname);
  121 |     if (method === 'DELETE' && delMatch) {
  122 |       const id = delMatch[1];
  123 |       const idx = store.findIndex((p) => p.id === id);
  124 |       if (idx !== -1) store.splice(idx, 1);
  125 |       await route.fulfill({ status: 204, body: '' });
  126 |       return;
  127 |     }
  128 | 
  129 |     // GET /v1/personas?tenant_id=...
  130 |     if (method === 'GET') {
  131 |       const tenant = url.searchParams.get('tenant_id');
  132 |       const filtered = store.filter((p) => p.tenant_id === tenant && !p.archived);
  133 |       await route.fulfill({
  134 |         status: 200,
  135 |         contentType: 'application/json',
  136 |         body: JSON.stringify(filtered),
  137 |       });
  138 |       return;
  139 |     }
  140 | 
  141 |     await route.continue();
  142 |   });
  143 | 
  144 |   return { getStore: () => store };
  145 | }
  146 | 
  147 | test.describe('admin-ui Personas pane — CRUD against mocked /v1/personas', () => {
  148 |   test('list renders mocked personas after tenant id entered', async ({ page }) => {
  149 |     await installPersonaMocks(page, [
  150 |       makePersona({ id: 'prs_alpha', name: 'Alpha Planner' }),
  151 |       makePersona({ id: 'prs_beta', name: 'Beta Worker' }),
  152 |     ]);
  153 | 
  154 |     await page.goto('/personas');
  155 |     // Seed the tenant id input so the first list call fires.
  156 |     await page.locator('input[type="text"]').first().fill(TENANT_ID);
  157 | 
  158 |     // The table should render with both rows.
  159 |     await expect(page.locator('table[aria-label="personas"]')).toBeVisible({
  160 |       timeout: 10_000,
  161 |     });
  162 |     await expect(page.locator('td', { hasText: 'Alpha Planner' })).toBeVisible();
  163 |     await expect(page.locator('td', { hasText: 'Beta Worker' })).toBeVisible();
  164 |   });
  165 | 
  166 |   test('name filter narrows visible rows', async ({ page }) => {
  167 |     await installPersonaMocks(page, [
  168 |       makePersona({ id: 'prs_a', name: 'Alpha Planner' }),
  169 |       makePersona({ id: 'prs_b', name: 'Beta Worker' }),
  170 |     ]);
  171 | 
  172 |     await page.goto('/personas');
  173 |     await page.locator('input[type="text"]').first().fill(TENANT_ID);
  174 |     await expect(page.locator('td', { hasText: 'Alpha Planner' })).toBeVisible();
  175 | 
  176 |     // Type "alpha" into the search input.
  177 |     await page.locator('input[type="search"]').fill('alpha');
  178 | 
  179 |     await expect(page.locator('td', { hasText: 'Alpha Planner' })).toBeVisible();
  180 |     await expect(page.locator('td', { hasText: 'Beta Worker' })).toHaveCount(0);
  181 |   });
  182 | 
  183 |   test('"New persona" drawer opens, submits, list refreshes', async ({ page }) => {
  184 |     await installPersonaMocks(page, []);
> 185 |     await page.goto('/personas');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/personas
  186 |     await page.locator('input[type="text"]').first().fill(TENANT_ID);
  187 | 
  188 |     // Wait for empty state then click the new-persona button.
  189 |     const newBtn = page.locator('button', { hasText: /new/i }).first();
  190 |     await expect(newBtn).toBeVisible({ timeout: 10_000 });
  191 |     await newBtn.click();
  192 | 
  193 |     // Drawer opens.
  194 |     const dialog = page.locator('[role="dialog"]');
  195 |     await expect(dialog).toBeVisible();
  196 | 
  197 |     // Fill name + system prompt + submit.
  198 |     await dialog.locator('input').first().fill('Created Persona');
  199 |     await dialog.locator('textarea').first().fill('role/planner');
  200 |     await dialog.locator('button[type="submit"]').click();
  201 | 
  202 |     // List refreshes — drawer closes and new row appears.
  203 |     await expect(dialog).not.toBeVisible({ timeout: 5_000 });
  204 |     await expect(page.locator('td', { hasText: 'Created Persona' })).toBeVisible({
  205 |       timeout: 5_000,
  206 |     });
  207 |   });
  208 | 
  209 |   test('edit drawer shows existing values', async ({ page }) => {
  210 |     await installPersonaMocks(page, [
  211 |       makePersona({
  212 |         id: 'prs_edit',
  213 |         name: 'Editable Persona',
  214 |         system_prompt: 'role/worker behaviour',
  215 |         default_model: 'gpt-4o',
  216 |       }),
  217 |     ]);
  218 | 
  219 |     await page.goto('/personas');
  220 |     await page.locator('input[type="text"]').first().fill(TENANT_ID);
  221 | 
  222 |     const editBtn = page.locator('button[aria-label="edit Editable Persona"]');
  223 |     await expect(editBtn).toBeVisible({ timeout: 10_000 });
  224 |     await editBtn.click();
  225 | 
  226 |     const dialog = page.locator('[role="dialog"]');
  227 |     await expect(dialog).toBeVisible();
  228 |     // Name input is pre-filled.
  229 |     await expect(dialog.locator('input').first()).toHaveValue('Editable Persona');
  230 |     // System prompt textarea is pre-filled.
  231 |     await expect(dialog.locator('textarea').first()).toHaveValue(
  232 |       'role/worker behaviour',
  233 |     );
  234 |   });
  235 | 
  236 |   test('delete confirm modal removes the row', async ({ page }) => {
  237 |     await installPersonaMocks(page, [
  238 |       makePersona({ id: 'prs_doomed', name: 'Doomed Persona' }),
  239 |     ]);
  240 | 
  241 |     await page.goto('/personas');
  242 |     await page.locator('input[type="text"]').first().fill(TENANT_ID);
  243 | 
  244 |     const deleteBtn = page.locator('button[aria-label="delete Doomed Persona"]');
  245 |     await expect(deleteBtn).toBeVisible({ timeout: 10_000 });
  246 |     await deleteBtn.click();
  247 | 
  248 |     // Confirmation modal appears (also role=dialog). Click the Delete button
  249 |     // inside the confirmation dialog (last dialog opened).
  250 |     const confirmDialog = page.locator('[role="dialog"]').last();
  251 |     await expect(confirmDialog).toBeVisible();
  252 |     await expect(confirmDialog.locator('strong', { hasText: 'Doomed Persona' }))
  253 |       .toBeVisible();
  254 |     // The dialog has Cancel + Delete buttons; click Delete.
  255 |     await confirmDialog.locator('button').last().click();
  256 | 
  257 |     // Row disappears.
  258 |     await expect(
  259 |       page.locator('td', { hasText: 'Doomed Persona' }),
  260 |     ).toHaveCount(0, { timeout: 5_000 });
  261 |   });
  262 | });
  263 | 
```