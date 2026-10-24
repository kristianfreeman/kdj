# build123d (Python CAD on OpenCascade) for the enclosure: reads STEP parts,
# checks collisions, exports GLB for the viewer and STL/STEP for printing.
FROM python:3.12-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends libgl1 libglib2.0-0 libxrender1 >/dev/null && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir -q build123d==0.13.0
WORKDIR /w
