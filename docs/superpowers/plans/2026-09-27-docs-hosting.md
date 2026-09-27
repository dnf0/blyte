# Host the Docs Site — Plan

**Status:** planned 2026-09-27; nothing built or provisioned yet. Decision so far (user): host
**internally first**, on Climate X's own web space via a **static S3 site**, to promote blyte inside
the company; open-source hosting comes later, with the repository going public
([rename plan](2026-09-27-rename-repos-to-blyte.md), phase 3).

**What is being hosted:** the MkDocs Material site built from `docs/` (`mkdocs build --strict`,
already run in CI): static HTML, CSS and SVG, no server side. Historical plans, specs and the
superseded ADRs are excluded from the build (`exclude_docs`).

**What the site contains** (checked 2026-09-27): no secrets, account ids, AWS profile names or
personal details. It links to the private repositories (`climate-x-org/superstate`,
`dnfcx/superstate-eval`), which 404 for anyone outside, and the FrontierHarness write-up mentions
that "the account role used here lacked `bedrock:InvokeModel`". The exposure of a readable-by-link
site is timing (an unreleased project), not secrets.

---

## Open decisions (before phase 1)

1. **Where:** the existing Climate X web space (which account, bucket or domain), or a new bucket.
2. **Who can read it:**
   - *unlisted* (anyone with the URL; kept out of search engines), the simplest; or
   - *staff-only*: CloudFront with a WAF allowlist of office/VPN ranges, or SSO in front
     (Cognito or the company identity provider via an edge function), or Amplify Hosting with a
     password.
3. **URL:** a CloudFront domain, or a subdomain of the company domain (for example
   `blyte.<company domain>`), which needs a DNS record from whoever owns it.

## Phase 1: internal site (S3 + CloudFront)

1. **Internal build safeguards** (repo, one PR): a `robots.txt` that disallows everything and a
   `<meta name="robots" content="noindex">` on every page, both switched on by a build flag
   (for example `BLYTE_DOCS_INTERNAL=1`, via a MkDocs theme override), so the public build later
   drops them by leaving the flag off.
2. **Infrastructure as code** (for the AWS owner to review and apply; agents never apply it):
   - a private S3 bucket (no public access, versioning on), served only through CloudFront with
     Origin Access Control;
   - a CloudFront distribution with HTTPS (ACM certificate in `us-east-1` if a custom domain is
     used), default root object `index.html`, and a small function that maps `/path/` to
     `/path/index.html` (MkDocs directory URLs);
   - the chosen access control from decision 2;
   - a GitHub OIDC identity provider and a deploy role that can only `s3:PutObject`,
     `s3:DeleteObject` and `s3:ListBucket` on this bucket and `cloudfront:CreateInvalidation` on
     this distribution, trusted only for this repository's `main` branch.
   Written in the form the account already uses (OpenTofu or CloudFormation), under
   `infra/docs-site/`.
3. **Deploy job** (repo, one PR): on merge to `main`, build with the internal flag, then
   `aws s3 sync site/ s3://<bucket>/ --delete` and a CloudFront invalidation, authenticated with the
   OIDC role (`aws-actions/configure-aws-credentials`), so no AWS keys are stored in GitHub. Off
   until the role ARN, bucket and distribution id are set as repository variables.
4. **Promote:** share the URL in internal channels only.

Cost: pennies a month (storage and requests for a few MB, CloudFront transfer at internal volumes).

## Phase 2: public site (with the open-source release)

When the repository goes public (rename plan, phase 3, after the history scan and company sign-off):

1. Remove the internal safeguards by building without the flag.
2. Choose the public home:
   - **keep S3 + CloudFront** with a public domain (the same pipeline, the access control removed);
     or
   - **move to GitHub Pages** (free for a public repository; `actions/deploy-pages` on merge), with
     a custom domain so the URL carries no `github.io` path: a `docs/CNAME` file, `site_url` in
     `mkdocs.yml`, the DNS record (`CNAME` to `climate-x-org.github.io`, or GitHub's `A` records for
     an apex domain), "Enforce HTTPS", and the domain verified for the organisation.
3. Update the links that name the site (README, crate metadata `homepage`, `mkdocs.yml`
   `site_url`) and, if the site moved, redirect the internal URL to the public one.
