FROM oven/bun:1.3.14-slim

WORKDIR /app
COPY server.ts store.ts schema.sql ./
RUN mkdir -p /data && chmod 700 /data

ENV NODE_ENV=production \
    NOTED_TEAM_HOST=0.0.0.0 \
    NOTED_TEAM_DB=/data/team.sqlite

EXPOSE 8790
ENTRYPOINT ["/bin/sh", "-c", "umask 077; exec bun /app/server.ts"]
