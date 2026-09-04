# Oga landing page

Cloudflare Pages serves this directory as a static site. It has no dependencies, build command, or generated output.

## Deploy from the command line

Install Wrangler once with `bun add --dev wrangler`, then deploy the landing page with:

```bash
make deploy-landing
```

Authenticate once with `bunx wrangler login`. Wrangler keeps the session, so later deploys need nothing else.

Before the first deployment, create the `oga` Pages project and set its production branch to `main` using the dashboard steps below. Configure `oga.desgn.space` there once. Later deployments only need `make deploy-landing`.

## Project settings

- Project name: `oga`
- Production branch: `main`
- Build command: leave blank
- Build output directory: `landing`
- Custom domain: `oga.desgn.space`

## Connect the repository

1. Open the Cloudflare dashboard and select **Workers & Pages**.
2. Select **Create application**, then **Pages**, then **Connect to Git**.
3. Choose the Git provider and authorize access to this repository if Cloudflare does not already have it.
4. Select this repository and choose **Begin setup**.
5. Enter `oga` as the project name and select `main` as the production branch.
6. Leave the framework preset unset and the build command blank.
7. Enter `landing` as the build output directory.
8. Select **Save and Deploy**.
9. Open the new Pages project, select **Custom domains**, then **Set up a custom domain**.
10. Enter `oga.desgn.space` and follow Cloudflare's DNS confirmation flow.

Cloudflare will deploy updates from `main`. Preview deployments are created for pull requests by default.

## Publish the Homebrew tap

The GitHub repository must be named exactly `DesgnSpace/homebrew-tap`. Homebrew maps `DesgnSpace/tap` to that repository. An empty repository is not a working tap: `Casks/oga.rb` must be committed and pushed before anyone can install Oga.

1. Create the `DesgnSpace/homebrew-tap` repository on GitHub.
2. Clone that repository beside this one.
3. Create a `Casks` directory in the tap repository.
4. Copy this repository's `Casks/oga.rb` to `homebrew-tap/Casks/oga.rb`.
5. Commit and push the cask from the tap repository.

Users can then install Oga with:

```bash
brew install --cask DesgnSpace/tap/oga
```

Each Oga release regenerates `Casks/oga.rb` with the new version and ZIP checksum. After publishing a release, copy the regenerated file to `homebrew-tap/Casks/oga.rb`, then commit and push it from the tap repository. Homebrew will read the updated cask on the next install or upgrade.
