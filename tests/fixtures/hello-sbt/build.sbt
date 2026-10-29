ThisBuild / scalaVersion := "2.13.16"

// Writes to the JVM's real stdout and stderr through a subprocess, the way a
// build plugin shelling out to a tool does: this output never becomes a
// JSON-RPC notification and must be captured from the pipes (gotcha 5).
lazy val shout = taskKey[Unit]("echo to the real stdout/stderr via a subprocess")

lazy val root = (project in file("."))
  .settings(
    name := "hello-sbt",
    shout := {
      val process = new ProcessBuilder(
        "sh",
        "-c",
        "echo raw-from-subprocess; echo raw-from-subprocess-err >&2"
      ).inheritIO().start()
      process.waitFor()
      ()
    }
  )
