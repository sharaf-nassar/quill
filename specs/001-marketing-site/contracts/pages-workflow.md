# GitHub Pages Workflow Contract — `.github/workflows/pages.yml`

Defines the deploy contract: which inputs trigger a build, which permissions are granted, what the artifact contains, where the deployment lands.

## Triggers

```yaml
on:
  push:
    branches: [main]
    paths:
      - "marketing-site/**"
      - ".github/workflows/pages.yml"
  workflow_dispatch:
```

- **`push`** on the default branch with a paths filter: only changes inside `marketing-site/` (or to the workflow file itself) trigger a deploy. Quill app changes do NOT trigger Pages workflows, saving CI minutes (FR-026 budget discipline at the project level).
- **`workflow_dispatch`** lets a maintainer manually re-run the workflow without pushing — useful when only screenshots in an asset bucket changed and a forced redeploy is needed.

## Permissions

```yaml
permissions:
  contents: read
  pages: write
  id-token: write
```

Least-privilege per GitHub's official Pages template. No write access to repository contents.

## Concurrency

```yaml
concurrency:
  group: "pages"
  cancel-in-progress: false
```

A second `pages` deploy waits in queue rather than canceling the first — matches GitHub's recommended pattern (an in-flight Pages deploy should not be killed mid-deploy).

## Jobs

### `build`

```yaml
build:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: actions/configure-pages@v5
    - uses: actions/upload-pages-artifact@v3
      with:
        path: marketing-site/
```

- No build step (per research R1). The artifact is the contents of `marketing-site/` verbatim.
- `actions/configure-pages@v5` is required so the artifact knows the deployment URL prefix.

### `deploy`

```yaml
deploy:
  needs: build
  runs-on: ubuntu-latest
  environment:
    name: github-pages
    url: ${{ steps.deployment.outputs.page_url }}
  steps:
    - id: deployment
      uses: actions/deploy-pages@v4
```

- Surfaces the deployed URL in the Actions UI (`environment.url`), so a maintainer can click straight from a green check to the live site.
- Two-job split is canonical Pages flow.

## Outputs

- A successful run produces a deployment to `https://<owner>.github.io/<repo>/` (or the configured custom domain — out of scope for v1 per spec).
- A failed run is visible as a red Actions check on the merge commit. No silent failure.

## Out-of-scope (deliberately)

- No Lighthouse CI step. v1 keeps the workflow minimal; perf verification is a manual pre-merge check (research R8).
- No HTML / link validation. Could be added later by inserting a validation step between checkout and upload.
- No preview deployments per pull request. Feasible later by adding a separate workflow with `pull_request` trigger and the `actions/deploy-pages` preview-environment knob, but out of scope now.

## Test surface

- After first merge, verify (a) the workflow ran on the merge commit, (b) the Pages environment shows a deployed URL, (c) the URL serves the expected `<title>` and `#hero` section.
