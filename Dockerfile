# docker build -t bpowers/model-service:latest .

# first compile our TypeScript
FROM node:12-alpine as builder

RUN apk update \
 && apk upgrade \
 && apk add --no-cache --virtual .build-deps git perl python make g++ cairo-dev pixman-dev pango-dev libjpeg-turbo-dev \
 && rm -rf /var/cache/apk/*

RUN npm install -g yarn

WORKDIR /model

COPY package.json yarn.lock ./

RUN yarn install

COPY . .

# in prod, for now, don't publish sourcemaps
ENV GENERATE_SOURCEMAP false

RUN yarn build

# next build the /node_env environment we'll run in production
FROM node:12-alpine as prod-node-modules

RUN apk update \
 && apk upgrade \
 && apk add --no-cache --virtual .build-deps git perl python make g++ cairo-dev pixman-dev pango-dev libjpeg-turbo-dev \
 && rm -rf /var/cache/apk/*

RUN npm install -g yarn

WORKDIR /model

COPY package.json yarn.lock ./

RUN yarn install

# do it this way to take advantage of the cached `yarn install` from the "builder" image above
ENV NODE_ENV production
RUN rm -rf node_modules; yarn install

# finally put the production container together
FROM node:12-alpine

RUN apk update \
 && apk upgrade \
 && apk add --no-cache ca-certificates cairo pango pixman libjpeg-turbo \
 && rm -rf /var/cache/apk/*

ENV NODE_ENV production

WORKDIR /model

COPY package.json yarn.lock ./

COPY --from=prod-node-modules /model/node_modules ./node_modules/

# backend
COPY --from=builder /model/fonts ./fonts/
COPY --from=builder /model/lib ./lib/
COPY --from=builder /model/config ./config/
COPY --from=builder /model/default_projects ./default_projects/
# frontend (compiled with Webpack)
COPY --from=builder /model/build ./public/
# web component (compiled with Webpack)
COPY --from=builder /model/build-component/static/js/sd-component.js ./public/static/js/

CMD [ "node", "lib" ]
