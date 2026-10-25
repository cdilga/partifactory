# Small, deliberately faulty fixtures; never publish these as release images.
ARG GOOD_IMAGE
ARG BUSYBOX_IMAGE
FROM ${BUSYBOX_IMAGE} AS tools
FROM ${GOOD_IMAGE} AS original
FROM original
ARG FAULT_MODE
COPY --from=original /partifactory /partifactory-real
COPY --from=tools /bin/busybox /busybox
COPY --chmod=0755 fault-entrypoint.sh /partifactory
ENV PF_FAULT_MODE=${FAULT_MODE}
LABEL pf.harness=1 pf.fixture=deploy-fault
