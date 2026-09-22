# OpenMayhem Core 0.2.253

Provider admission now treats an accepted reservation awaiting canonical state
as recoverable work. An identical session replay resumes the same signed
reservation, while changed requests and terminal rejections remain rejected.

Admission timeouts retain the exact reservation identity for recovery instead
of reporting a balance failure. This prevents delayed writer acknowledgements
from making an available provider appear unavailable.

Contract version 26 and its authenticated history are unchanged.
