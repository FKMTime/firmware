FROM ubuntu/bind9:latest
RUN apt update && apt install -y gettext-base && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY ./entrypoint.sh .

COPY named.conf .
COPY *.zone .

ENTRYPOINT ["/app/entrypoint.sh"]
