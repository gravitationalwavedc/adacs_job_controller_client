#!/bin/bash

export DOCKER_BUILDKIT=1

# Test build using docker-compose override file
docker-compose -f docker/docker-compose.yaml -f docker/docker-compose.valgrind.yaml build
docker-compose -f docker/docker-compose.yaml -f docker/docker-compose.valgrind.yaml run web /runvalgrind.sh

# Clean up
docker-compose -f docker/docker-compose.yaml -f docker/docker-compose.valgrind.yaml stop
docker-compose -f docker/docker-compose.yaml -f docker/docker-compose.valgrind.yaml down
docker-compose -f docker/docker-compose.yaml -f docker/docker-compose.valgrind.yaml rm -fs db
docker volume rm docker_var_lib_mysql_job_server_valgrind
