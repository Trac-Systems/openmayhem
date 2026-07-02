FROM node:22-bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    libatomic1 \
    libgcc-s1 \
    libstdc++6 \
  && rm -rf /var/lib/apt/lists/*

RUN groupadd --system ops \
  && useradd --system --gid ops --home /home/ops --shell /usr/sbin/nologin ops \
  && mkdir -p /home/ops \
  && chown ops:ops /home/ops \
  && mkdir -p /msb/stores \
  && chown -R node:ops /msb

WORKDIR /msb

USER node

COPY package*.json ./
RUN npm ci --omit=dev
COPY . .

USER root
RUN chown -R node:ops /msb \
  && chmod -R 0555 /msb \
  && chown -R node:node /msb/stores \
  && chmod 0700 /msb/stores

USER node

ENV MSB_STORE=node-store \
    MSB_HOST=0.0.0.0 \
    MSB_PORT=5000

VOLUME ["/msb/stores"]
EXPOSE 5000

ENTRYPOINT ["npm", "run"]
CMD ["env-prod-rpc-docker"]
