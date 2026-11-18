# Testrix Cloudflare Pages (Private Viewer)

A zero-dependency Cloudflare Pages + Pages Functions + R2 viewer for Testrix YAML projects.
No Node.js build step, no `package.json`, and no `node_modules` required.

## Setup

1. Create an R2 bucket in Cloudflare (e.g. `testrix-share`).
2. Deploy the `testrix-cloudflare` folder to **Cloudflare Pages** (via Git integration or Cloudflare dashboard direct upload / `wrangler pages deploy public`).
3. Bind your R2 bucket as `TESTRIX_BUCKET` in `wrangler.toml` or in the Cloudflare Pages dashboard (**Settings → Functions → R2 bucket bindings**).
4. (Optional) Protect the site with **Cloudflare Zero Trust / Access**, set `ACCESS_PASSCODE` in Pages environment variables, or set an **Access Token** in **Testrix App → Preferences → Share Access Control**.
5. In **Testrix App → Preferences → Cloudflare R2**, enter your Cloudflare Account ID, R2 Bucket Name, API Token, and this Pages URL, then use any project's **Share** tab to upload its YAML files to R2.
