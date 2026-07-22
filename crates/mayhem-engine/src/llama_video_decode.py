import av
import struct
import sys


def fail(message):
    raise ValueError(message)


requested = int(sys.argv[1])
fps = float(sys.argv[2])
max_input_bytes = int(sys.argv[3])
max_decoded_bytes = int(sys.argv[4])
max_dimension = int(sys.argv[5])

if requested < 1 or requested > 64:
    fail("requested frame count must be between 1 and 64")
if fps <= 0:
    fail("sampling fps must be positive")

payload = sys.stdin.buffer.read(max_input_bytes + 1)
if not payload or len(payload) > max_input_bytes:
    fail("video container exceeds the bounded input size")

container = av.open(__import__("io").BytesIO(payload))
try:
    stream = container.streams.video[0]
    total = int(stream.frames or 0)
    if total > 0:
        if requested == 1:
            wanted = [0]
        else:
            wanted = [
                (index * (total - 1)) // (requested - 1)
                for index in range(requested)
            ]
        wanted_counts = {}
        for index in wanted:
            wanted_counts[index] = wanted_counts.get(index, 0) + 1
    else:
        wanted = None
        wanted_counts = None

    emitted = 0
    decoded_bytes = 0
    next_time = 0.0
    source_rate = float(stream.average_rate) if stream.average_rate else 0.0
    last_frame = None

    def emit_frame(frame):
        global emitted, decoded_bytes, last_frame
        rgb = frame.reformat(format="rgb24")
        width = int(rgb.width)
        height = int(rgb.height)
        if width < 1 or height < 1 or width > max_dimension or height > max_dimension:
            fail("decoded frame dimensions exceed the bounded limit")
        plane = rgb.planes[0]
        row_bytes = width * 3
        raw = bytes(plane)
        packed = b"".join(
            raw[row * plane.line_size : row * plane.line_size + row_bytes]
            for row in range(height)
        )
        if len(packed) != row_bytes * height:
            fail("decoded frame has an invalid RGB layout")
        decoded_bytes += len(packed)
        if decoded_bytes > max_decoded_bytes:
            fail("decoded frames exceed the bounded output size")
        sys.stdout.buffer.write(struct.pack(">III", width, height, len(packed)))
        sys.stdout.buffer.write(packed)
        emitted += 1
        last_frame = (width, height, packed)

    for index, frame in enumerate(container.decode(stream)):
        if wanted_counts is not None:
            copies = wanted_counts.get(index, 0)
        else:
            frame_time = frame.time
            if frame_time is None and source_rate > 0:
                frame_time = index / source_rate
            if frame_time is None:
                selected = emitted < requested
            else:
                selected = frame_time + 1e-9 >= next_time
                if selected:
                    next_time += 1.0 / fps
            copies = 1 if selected else 0
        if copies == 0:
            continue
        for _ in range(copies):
            emit_frame(frame)
        if emitted == requested:
            break

    while emitted < requested and last_frame is not None:
        width, height, packed = last_frame
        decoded_bytes += len(packed)
        if decoded_bytes > max_decoded_bytes:
            fail("decoded frames exceed the bounded output size")
        sys.stdout.buffer.write(struct.pack(">III", width, height, len(packed)))
        sys.stdout.buffer.write(packed)
        emitted += 1
finally:
    container.close()

if emitted != requested:
    fail("video did not yield the requested number of frames")
