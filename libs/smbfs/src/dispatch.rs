use crate::{Backend, Namespace, O_CREAT, O_EXCL};
use ufs_abi::{
    payload::{Attr, Reader, SetTimes, Writer},
    Op, Request, MAX_DATA,
};

fn name<'a>(r: &mut Reader<'a>) -> Result<&'a str, i32> {
    std::str::from_utf8(r.name().map_err(|_| 22)?).map_err(|_| 22)
}
fn attr(a: Attr) -> Result<Vec<u8>, i32> {
    let mut b = vec![0; Attr::SIZE];
    a.encode(&mut b).map_err(|_| 22)?;
    Ok(b)
}

impl<B: Backend> Namespace<B> {
    pub fn serve(&mut self, req: &Request<'_>, now: u64) -> Result<Vec<u8>, i32> {
        let mut r = Reader::new(req.payload);
        match req.op {
            Op::Lookup => {
                let n = name(&mut r)?;
                r.finish().map_err(|_| 22)?;
                attr(self.lookup(req.node, n, now)?)
            }
            Op::Getattr => {
                r.finish().map_err(|_| 22)?;
                attr(self.getattr(req.node, now)?)
            }
            Op::Parent => {
                r.finish().map_err(|_| 22)?;
                let (p, n) = self.parent(req.node)?;
                let mut b = vec![0; 10 + n.len()];
                let mut w = Writer::new(&mut b);
                w.u64(p).map_err(|_| 22)?;
                w.name(n.as_bytes()).map_err(|_| 22)?;
                Ok(b)
            }
            Op::Setattr => {
                let (size, times) = if req.payload.len() == SetTimes::SIZE {
                    (None, Some(SetTimes::decode(req.payload).map_err(|_| 22)?))
                } else {
                    let s = r.u64().map_err(|_| 22)?;
                    r.finish().map_err(|_| 22)?;
                    (Some(s), None)
                };
                attr(self.setattr(req.node, size, times, now)?)
            }
            Op::Create | Op::Mkdir => {
                let _mode = r.u32().map_err(|_| 22)?;
                let n = name(&mut r)?;
                r.finish().map_err(|_| 22)?;
                attr(if req.op == Op::Create {
                    self.create(req.node, n, O_CREAT | O_EXCL, now)?
                } else {
                    self.mkdir(req.node, n, now)?
                })
            }
            Op::Open => {
                let flags = r.u32().map_err(|_| 22)?;
                r.finish().map_err(|_| 22)?;
                Ok(self.open(req.node, flags)?.to_le_bytes().to_vec())
            }
            Op::Read => {
                let h = r.u64().map_err(|_| 22)?;
                let off = r.u64().map_err(|_| 22)?;
                let len = r.u32().map_err(|_| 22)? as usize;
                r.finish().map_err(|_| 22)?;
                if len > MAX_DATA {
                    return Err(22);
                }
                self.read(h, off, len)
            }
            Op::Write => {
                let h = r.u64().map_err(|_| 22)?;
                let off = r.u64().map_err(|_| 22)?;
                let data = r.rest();
                Ok((self.write(h, off, data)? as u32).to_le_bytes().to_vec())
            }
            Op::Readdir => {
                let cookie = r.u64().map_err(|_| 22)?;
                r.finish().map_err(|_| 22)?;
                let entries = self.readdir(req.node)?;
                let mut b = vec![0; MAX_DATA];
                let mut w = Writer::new(&mut b);
                for (i, e) in entries
                    .iter()
                    .filter(|e| e.name != "." && e.name != "..")
                    .enumerate()
                {
                    if (i as u64) < cookie {
                        continue;
                    }
                    if w.len() + 22 + e.name.len() > MAX_DATA {
                        break;
                    }
                    let a = self.lookup(req.node, &e.name, now)?;
                    w.u64(i as u64 + 1).map_err(|_| 22)?;
                    w.u64(a.node).map_err(|_| 22)?;
                    w.u32(a.mode).map_err(|_| 22)?;
                    w.name(e.name.as_bytes()).map_err(|_| 22)?;
                }
                let n = w.len();
                b.truncate(n);
                Ok(b)
            }
            Op::Unlink | Op::Rmdir => {
                let n = name(&mut r)?;
                r.finish().map_err(|_| 22)?;
                if req.op == Op::Unlink {
                    self.unlink(req.node, n)?
                } else {
                    self.rmdir(req.node, n)?
                }
                Ok(vec![])
            }
            Op::Rename => {
                let p = r.u64().map_err(|_| 22)?;
                let from = name(&mut r)?;
                let to = name(&mut r)?;
                r.finish().map_err(|_| 22)?;
                self.rename(req.node, from, p, to)?;
                Ok(vec![])
            }
            Op::Fsync | Op::Release => {
                let h = r.u64().map_err(|_| 22)?;
                r.finish().map_err(|_| 22)?;
                if req.op == Op::Fsync {
                    self.fsync(h)?
                } else {
                    self.release(h)?
                }
                Ok(vec![])
            }
            Op::Statfs => {
                r.finish().map_err(|_| 22)?;
                let s = self.statfs(req.node)?;
                let mut b = vec![0; 56];
                let mut w = Writer::new(&mut b);
                for n in [
                    s.bsize, s.blocks, s.bfree, s.bavail, s.files, s.ffree, s.namelen,
                ] {
                    w.u64(n).map_err(|_| 22)?;
                }
                Ok(b)
            }
        }
    }
}
