# Financial Ratio Reference Guide

Source: Original work released under MIT License by xiaoguai contributors
License: MIT
Corpus role: Ratio definitions, formulas, benchmarks — used by ratio-extract prompt

---

## 1. Profitability Ratios

### Gross Profit Margin
**Formula**: (Revenue - Cost of Goods Sold) / Revenue × 100
**Also called**: Gross margin
**Interpretation**: Measures how efficiently the company produces its products/services.
Higher is better. Software/SaaS companies typically 60-80%; manufacturing 20-40%.
**Benchmark**: Above 40% is generally strong across most sectors.

### Operating Profit Margin (EBIT Margin)
**Formula**: Operating Income / Revenue × 100
**Also called**: EBIT margin, operating margin
**Interpretation**: Measures operating efficiency before interest and taxes. Excludes
financing structure effects, enabling comparison across companies with different capital structures.
**Benchmark**: Technology sector ~20-30%; retail ~3-7%.

### Net Profit Margin
**Formula**: Net Income / Revenue × 100
**Interpretation**: Bottom-line profitability after all expenses including interest and tax.
**Benchmark**: Highly variable; 10%+ is strong for most industries.

### Return on Assets (ROA)
**Formula**: Net Income / Average Total Assets × 100
**Interpretation**: How efficiently the company uses its assets to generate profit.
**Benchmark**: Above 5% is generally good; financial companies 1-2% is typical.

### Return on Equity (ROE)
**Formula**: Net Income / Average Stockholders' Equity × 100
**Also called**: Return on net worth
**Interpretation**: Return generated on shareholders' invested capital. DuPont decomposition:
ROE = Net Margin × Asset Turnover × Equity Multiplier.
**Benchmark**: 15-20%+ indicates strong returns; compare to sector peers.

### Return on Invested Capital (ROIC)
**Formula**: NOPAT / Invested Capital × 100
Where NOPAT = Net Operating Profit After Tax = EBIT × (1 - tax rate)
Where Invested Capital = Total Equity + Total Debt - Cash and Short-term Investments
**Interpretation**: Measures returns relative to all capital deployed (debt + equity). Best
measure of economic value creation; compare to WACC. ROIC > WACC = value creation.

### EBITDA Margin
**Formula**: EBITDA / Revenue × 100
Where EBITDA = Earnings Before Interest, Taxes, Depreciation, and Amortization
**Interpretation**: Proxy for operating cash generation. Strips out non-cash charges.
Often used in valuation (EV/EBITDA multiple). Note: EBITDA is not a GAAP measure.

---

## 2. Liquidity Ratios

### Current Ratio
**Formula**: Current Assets / Current Liabilities
**Interpretation**: Measures short-term solvency. Ratio > 1 means current assets exceed
current liabilities. Ratio < 1 signals potential liquidity stress.
**Benchmark**: Generally 1.5-3.0x is healthy; below 1.0x is a red flag.

### Quick Ratio (Acid-Test)
**Formula**: (Cash + Short-term Investments + Accounts Receivable) / Current Liabilities
**Interpretation**: More conservative than current ratio — excludes inventory and prepaid
expenses, which may be harder to liquidate quickly.
**Benchmark**: Above 1.0x is generally adequate.

### Cash Ratio
**Formula**: (Cash + Cash Equivalents) / Current Liabilities
**Interpretation**: Most conservative liquidity measure — only counts the most liquid assets.
**Benchmark**: 0.5-1.0x is typically sufficient; too high may indicate inefficient cash use.

### Operating Cash Flow Ratio
**Formula**: Operating Cash Flow / Current Liabilities
**Interpretation**: Whether the company's operations generate enough cash to cover near-term
obligations. Preferred over accrual ratios when assessing true liquidity.

---

## 3. Leverage / Solvency Ratios

### Debt-to-Equity (D/E) Ratio
**Formula**: Total Debt / Total Stockholders' Equity
**Note**: "Total Debt" = interest-bearing debt only (short-term + long-term); excludes AP,
deferred revenue, and other operating liabilities.
**Interpretation**: Measures financial leverage. Higher D/E = more risk to equity holders.
**Benchmark**: Below 1.0x is conservative; 1.0-2.0x moderate; above 3.0x high risk.

### Debt-to-Assets Ratio
**Formula**: Total Debt / Total Assets
**Interpretation**: Proportion of assets financed by debt. Lower is less risky.
**Benchmark**: Below 0.5 is generally conservative.

### Interest Coverage Ratio
**Formula**: EBIT / Interest Expense
**Also called**: Times interest earned
**Interpretation**: How many times the company can cover interest payments from operating
income. Below 1.5x is a red flag.
**Benchmark**: Above 3.0x is generally healthy; below 2.0x signals stress.

### Net Debt / EBITDA
**Formula**: (Total Debt - Cash and Equivalents) / EBITDA
**Also called**: Leverage ratio, net leverage
**Interpretation**: Common in credit analysis. Investment-grade companies typically maintain
below 2.5-3.0x; above 5.0x often signals distress.
**Benchmark**: Investment grade ≤ 2.5x; leveraged buyout targets 4-6x.

---

## 4. Efficiency / Activity Ratios

### Asset Turnover
**Formula**: Revenue / Average Total Assets
**Interpretation**: Revenue generated per dollar of assets. Higher = more efficient.
**Benchmark**: Capital-light businesses (software) often 0.5-1.0x; retailers 1.5-2.5x.

### Receivables Turnover
**Formula**: Revenue / Average Accounts Receivable
**Days Sales Outstanding (DSO)**: 365 / Receivables Turnover
**Interpretation**: How quickly customers pay. Higher turnover / lower DSO = faster collection.
**Benchmark**: DSO of 30-60 days is typical; above 90 days is a warning sign.

### Inventory Turnover
**Formula**: Cost of Goods Sold / Average Inventory
**Days Inventory Outstanding (DIO)**: 365 / Inventory Turnover
**Interpretation**: How efficiently inventory is managed. Higher turnover = lean operations.
**Benchmark**: Grocery retail 15-20x; manufacturing 4-8x.

### Payables Turnover
**Formula**: Cost of Goods Sold / Average Accounts Payable
**Days Payable Outstanding (DPO)**: 365 / Payables Turnover
**Interpretation**: How long the company takes to pay suppliers. Higher DPO conserves cash
but may strain supplier relationships if excessive.

### Cash Conversion Cycle (CCC)
**Formula**: DIO + DSO - DPO
**Interpretation**: Days from cash outflow (inventory purchase) to cash inflow (customer
payment). Negative CCC (e.g., Amazon) means the company collects from customers before it
pays suppliers — highly efficient.

---

## 5. Valuation Ratios (Requires Market Data)

### Price-to-Earnings (P/E) Ratio
**Formula**: Share Price / Earnings Per Share (EPS)
**Interpretation**: How much investors pay per dollar of earnings. Higher P/E implies higher
growth expectations or premium valuation.

### Enterprise Value / EBITDA (EV/EBITDA)
**Formula**: (Market Cap + Total Debt - Cash) / EBITDA
**Interpretation**: Used in M&A and sector comparisons. Less distorted by capital structure
than P/E. Technology sector 15-25x typical; industrials 8-12x.

### Price-to-Book (P/B) Ratio
**Formula**: Share Price / Book Value Per Share
**Interpretation**: Premium (or discount) to net asset value. P/B > 1 suggests the market
values the business above its accounting net assets (intangibles, brand, future earnings).
