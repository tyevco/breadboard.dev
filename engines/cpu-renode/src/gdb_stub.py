# Breadboard: Renode's GDB stub for bb's GDB proxy only (ADR-008 D10,
# Amendment 8). Run by the session script, in the monitor's Python engine:
#
#     python "exec(open('<this file>').read()); bb_gdb_stub(monitor.Machine, <port>, '<proxy>', '<log>', <wait ms>)"
#
# Renode 1.17.0's `machine StartGdbServer <port>` binds the stub on every
# interface (`SocketServerProvider.Start`: `IPAddress.Any`, backlog 1) and
# hands it to whichever client the listener accepts first. So the stub is
# not created up front. A `SocketServerProvider` of our own listens on the
# port, with this accept hook attached before it starts listening, so every
# accepted connection passes through it. The hook creates the stub
# (`machine.StartGdbServer(terminal, None)`, the variant for a terminal
# that already has its client) only for the connection whose remote end is
# the endpoint bb's proxy wrote to `<proxy>` after its connect returned
# (bb's private workdir). Any other connection is logged and shut down
# without ever reaching a stub. Each decision is one line of `<log>`:
#
#     ready <port>          listening
#     stub <addr>:<port>    the stub was created for this connection
#     refused <addr>:<port> shut down without a stub
#     error <text>          the hook failed (the connection is shut down)
#
# bb starts the run only after it has read `stub <its own endpoint>`.
import clr
clr.AddReference("System.Net.Primitives")
clr.AddReference("System.Net.Sockets")
from System import Action
from System.Diagnostics import Stopwatch
from System.IO import File, Stream
from System.Net import IPAddress
from System.Net.Sockets import SocketShutdown
from System.Threading import Thread
from Antmicro.Renode.Utilities import SocketServerProvider


def bb_gdb_stub(machine, port, proxy_file, log_file, wait_ms):
    terminal = SocketServerProvider(False, False, "GDB")
    state = {"stub": False}

    def log(line):
        # Never raise on the listener thread (an exception there ends
        # Renode); bb fails the run when its `stub` line does not come.
        try:
            f = open(log_file, "a")
            f.write(line + "\n")
            f.close()
        except Exception:
            pass

    def proxy_endpoint():
        watch = Stopwatch.StartNew()
        while not File.Exists(proxy_file):
            if watch.ElapsedMilliseconds >= wait_ms:
                return None
            Thread.Sleep(1)
        return File.ReadAllText(proxy_file).strip()

    def shut(stream):
        try:
            stream.Socket.Shutdown(SocketShutdown.Both)
        except Exception:
            pass

    def accepted(stream):
        # On the listener thread, before the connection's reader starts:
        # nothing the client sent has been read yet.
        try:
            ep = stream.Socket.RemoteEndPoint
            peer = "%s:%d" % (ep.Address, ep.Port)
            if (not state["stub"] and IPAddress.IsLoopback(ep.Address)
                    and proxy_endpoint() == peer):
                machine.StartGdbServer(terminal, None)
                state["stub"] = True
                log("stub " + peer)
            else:
                log("refused " + peer)
                shut(stream)
        except Exception as e:
            log("error %s" % str(e).replace("\n", " "))
            shut(stream)

    terminal.ConnectionAccepted += Action[Stream](accepted)
    terminal.Start(port)
    log("ready %d" % port)
    return terminal
