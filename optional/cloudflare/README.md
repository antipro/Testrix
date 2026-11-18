# Testrix Cloudflare Pages (`optional/cloudflare`)

A zero-dependency **Cloudflare Pages + Pages Functions + R2** private API documentation viewer for Testrix YAML projects.

- No Node.js build step, no webpack, no `package.json`, and no `node_modules` required.
- Renders projects with a left navigation tree (folders, HTTP method badges, search filter) and a right content pane (resolved URLs with environment selector, query params, headers, auth, body, and Copy cURL).
- Does **not** expose public registration or upload endpoints — projects are uploaded directly from **Testrix App** to your R2 bucket.

## Structure

- `public/` — Static HTML/CSS/JS viewer (`index.html`, `styles.css`, `app.js`)
- `functions/` — Cloudflare Pages Functions reading YAML files from the `TESTRIX_BUCKET` R2 binding:
  - `GET /api/projects` — lists projects in R2 (`<slug>/testrix.project.yaml`)
  - `GET /api/projects/:slug` — parses and returns a project's YAML tree
  - `POST /api/projects/:slug/unlock` — verifies per-project access token (if set)
  - `POST /api/auth` — verifies global `ACCESS_PASSCODE` (if set)
- `wrangler.toml` — Cloudflare Pages & R2 bucket binding configuration

## Deployment

1. **Create an R2 bucket** in your Cloudflare dashboard (e.g. `testrix-share`).
2. **Create a Cloudflare API Token** with **R2 Object Read & Write** permissions for that bucket/account.
3. **Deploy `optional/cloudflare` to Cloudflare Pages**:
   - **Git integration**: Set **Root directory** to `optional/cloudflare`, leave **Build command** empty, and set **Build output directory** to `public`.
   - **CLI (`wrangler`)**: Run `wrangler pages deploy public` from `optional/cloudflare`.
4. **Bind the R2 bucket**:
   - In Cloudflare Pages (**Settings → Functions → R2 bucket bindings**), bind variable name `TESTRIX_BUCKET` to your R2 bucket (`testrix-share`), or edit `bucket_name` in `wrangler.toml`.

## Access Control (Optional)

You can restrict access to your deployed Pages site in any of three ways:

1. **Cloudflare Zero Trust / Access** in front of the Pages domain (recommended for teams).
2. **Site-wide passcode**: Set the `ACCESS_PASSCODE` environment variable in Cloudflare Pages (**Settings → Environment variables**).
3. **Per-project Access Token**: Set an **Access Token** in **Testrix App → Preferences → Share Access Control** before uploading. Testrix App stores `<project-slug>/.testrix-access.json` in R2, and Pages Functions prompt visitors for that token before serving the project.

## Uploading & Revoking from Testrix App

1. Open **Testrix App → Preferences** and fill in **Cloudflare R2 Storage**:
   - **Cloudflare Account ID**
   - **R2 Bucket** (e.g. `testrix-share`)
   - **Cloudflare API Token**
   - **Pages Viewer URL** (optional, e.g. `https://your-site.pages.dev`)
2. Open any project → **Share** tab → select **Cloudflare R2 Storage** → click **Upload Project YAML**.
3. To revoke a shared project later, click **Shares** in the top toolbar → **Revoke** to delete its YAML files from R2.
