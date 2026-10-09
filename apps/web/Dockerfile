# syntax=docker/dockerfile:1
# Kooperatif web frontend — local pilot packaging (PILOT-FIX-001 F2).
#
# Build context MUST be the repository root: the app is a pnpm
# workspace member that depends on @kooperatif/contracts.
#
#   docker build -f apps/web/Dockerfile -t kooperatif-web:pilot \
#       --build-arg PUBLIC_API_BASE_URL=http://localhost:18080 .
#
# PUBLIC_API_BASE_URL is a BUILD-TIME value (import.meta.env): the
# browser-visible API origin is baked into the bundle; it is not
# configurable at container start.

FROM node:24-bookworm-slim AS build
RUN corepack enable && corepack prepare pnpm@10.15.0 --activate
WORKDIR /src

# Workspace manifests first for dependency layer caching.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/.npmrc apps/web/.npmrc
COPY packages/contracts/package.json packages/contracts/
COPY apps/web/package.json apps/web/
RUN pnpm install --frozen-lockfile --filter @kooperatif/web...

COPY packages/contracts packages/contracts
COPY apps/web apps/web
ARG PUBLIC_API_BASE_URL=http://localhost:18080
ENV PUBLIC_API_BASE_URL=${PUBLIC_API_BASE_URL}
RUN pnpm --filter @kooperatif/web build

# adapter-node externalizes runtime imports (clsx, tailwind-merge, …);
# pnpm deploy materializes a self-contained prod-only install at
# /pruned, workspace dependency included.
RUN pnpm --filter @kooperatif/web deploy --legacy --prod /pruned \
    && cp -r apps/web/build /pruned/build

FROM node:24-bookworm-slim AS runtime
RUN groupadd --system kooperatif && useradd --system --gid kooperatif kooperatif
WORKDIR /app
COPY --from=build --chown=kooperatif:kooperatif /pruned ./
USER kooperatif
ENV NODE_ENV=production HOST=0.0.0.0 PORT=3000
EXPOSE 3000
CMD ["node", "build"]
