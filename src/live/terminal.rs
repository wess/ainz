use anyhow::Result;
use std::{
  fs::File,
  os::fd::{AsRawFd, FromRawFd, OwnedFd},
  process::Stdio,
};
use tokio::{
  io::unix::AsyncFd,
  process::{Child, Command},
};

pub(super) struct Pty(AsyncFd<OwnedFd>);

pub(super) fn spawn(command: &mut Command, cols: u16, rows: u16) -> Result<(Pty, Child)> {
  let mut master = -1;
  let mut slave = -1;
  let mut size = size(cols, rows);
  // openpty initializes two descriptors and copies the supplied window size. Linux takes the
  // size as `*const` and macOS as `*mut`; `as _` fits either.
  if unsafe {
    libc::openpty(
      &mut master,
      &mut slave,
      std::ptr::null_mut(),
      std::ptr::null_mut(),
      &raw mut size as _,
    )
  } < 0
  {
    return Err(std::io::Error::last_os_error().into());
  }
  // Both descriptors were returned exclusively to this call.
  let master = unsafe { OwnedFd::from_raw_fd(master) };
  let slave = File::from(unsafe { OwnedFd::from_raw_fd(slave) });
  unsafe {
    if libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) < 0
      || libc::fcntl(slave.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) < 0
      || libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) < 0
    {
      return Err(std::io::Error::last_os_error().into());
    }
  }
  command
    .stdin(Stdio::from(slave.try_clone()?))
    .stdout(Stdio::from(slave.try_clone()?))
    .stderr(Stdio::from(slave));
  // Stdio is already installed on 0/1/2 when pre_exec runs; the slave becomes the worker's
  // controlling terminal without any relationship to the terminal that launched the host.
  unsafe {
    command.pre_exec(|| {
      if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
        return Err(std::io::Error::last_os_error());
      }
      Ok(())
    });
  }
  let child = command.spawn()?;
  Ok((Pty(AsyncFd::new(master)?), child))
}

fn size(cols: u16, rows: u16) -> libc::winsize {
  libc::winsize {
    ws_col: cols.clamp(1, 1000),
    ws_row: rows.clamp(1, 1000),
    ws_xpixel: 0,
    ws_ypixel: 0,
  }
}

impl Pty {
  pub(super) fn resize(&self, cols: u16, rows: u16) -> Result<()> {
    let size = size(cols, rows);
    if unsafe { libc::ioctl(self.0.get_ref().as_raw_fd(), libc::TIOCSWINSZ as _, &size) } < 0 {
      return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
  }

  pub(super) async fn read(&self) -> Result<Vec<u8>> {
    let mut bytes = vec![0; 8192];
    loop {
      let mut ready = self.0.readable().await?;
      match ready.try_io(|fd| {
        let count = unsafe {
          libc::read(
            fd.get_ref().as_raw_fd(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
          )
        };
        if count < 0 {
          Err(std::io::Error::last_os_error())
        } else {
          Ok(count as usize)
        }
      }) {
        Ok(Ok(count)) => {
          bytes.truncate(count);
          return Ok(bytes);
        }
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => continue,
      }
    }
  }

  pub(super) async fn write(&self, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
      let mut ready = self.0.writable().await?;
      match ready.try_io(|fd| {
        let count =
          unsafe { libc::write(fd.get_ref().as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        if count < 0 {
          Err(std::io::Error::last_os_error())
        } else {
          Ok(count as usize)
        }
      }) {
        Ok(Ok(0)) => anyhow::bail!("worker terminal closed"),
        Ok(Ok(count)) => bytes = &bytes[count..],
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => continue,
      }
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;

  #[tokio::test]
  async fn a_worker_keeps_its_terminal_and_window_size_without_an_attached_client() {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(
      "stty -echo; printf 'ready\\n'; read answer; stty size; printf 'answer:%s\\n' \"$answer\"",
    );
    let (pty, mut child) = spawn(&mut command, 90, 24).unwrap();
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
      while !output.contains("ready") {
        output.push_str(&String::from_utf8_lossy(&pty.read().await.unwrap()));
      }
    })
    .await
    .unwrap();
    pty.resize(72, 18).unwrap();
    pty.write(b"stillalive\n").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
      while !output.contains("answer:stillalive") {
        output.push_str(&String::from_utf8_lossy(&pty.read().await.unwrap()));
      }
    })
    .await
    .unwrap();
    assert!(output.contains("18 72"), "{output}");
    assert!(child.wait().await.unwrap().success());
  }
}
