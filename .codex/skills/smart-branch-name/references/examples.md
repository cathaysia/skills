# Examples

Use these examples to keep prefix selection and issue-key formatting consistent.

## Feature

- Request: `Add GitHub SSO login`
- No issue convention detected: `feature/github-sso-login`
- Issue convention detected and user provides `AUTH-12`: `feature/AUTH-12-github-sso-login`

## Bugfix

- Request: `Fix duplicate invoice emails`
- No issue convention detected: `bugfix/duplicate-invoice-emails`
- Issue convention detected and user provides `BILL-207`: `bugfix/BILL-207-duplicate-invoice-emails`

## Hotfix

- Request: `Urgent fix for production checkout crash`
- No issue convention detected: `hotfix/production-checkout-crash`
- Issue convention detected and user provides `OPS-9`: `hotfix/OPS-9-production-checkout-crash`

## Ambiguous

- Request: `Update checkout flow`
- Action: ask whether this is a feature, bugfix, or hotfix before suggesting the name

## Opt-out

- Request: `Add branch naming helper`
- Issue convention detected but user answers `none`
- Result: `feature/branch-naming-helper`
