#!/bin/bash

export DOCKER_BUILDKIT=1

# Standard build of the normal docker-compose file
mkdir -p ./out
docker-compose -f docker/docker-compose.yaml up --build
