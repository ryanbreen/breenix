use std::io::Read;
use std::os::unix::fs::MetadataExt;

pub struct MountOptions {
    pub host: String,
    pub share: String,
    pub credfile: String,
    pub port: u16,
    pub smb302: bool,
}
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub domain: String,
}

pub fn parse_mount_options(unc: &str, options: &str) -> Result<MountOptions, &'static str> {
    let path = unc.strip_prefix("//").ok_or("invalid share")?;
    let (host, share) = path.split_once('/').ok_or("invalid share")?;
    if host.is_empty()
        || share.is_empty()
        || share.contains('/')
        || host.contains(['\\', '\0'])
        || share.contains(['\\', '\0'])
    {
        return Err("invalid share");
    }
    let mut credfile = None;
    let mut port = None;
    let mut version = None;
    for item in options.split(',') {
        let (key, value) = item.split_once('=').ok_or("invalid option")?;
        match key {
            "credfile" if credfile.is_none() && !value.is_empty() => {
                credfile = Some(value.to_owned())
            }
            "port" if port.is_none() => {
                let p = value.parse::<u16>().map_err(|_| "invalid port")?;
                if p == 0 {
                    return Err("invalid port");
                }
                port = Some(p);
            }
            "vers" if version.is_none() && value == "3.0.2" => version = Some(true),
            _ => return Err("invalid option"),
        }
    }
    Ok(MountOptions {
        host: host.into(),
        share: share.into(),
        credfile: credfile.ok_or("missing credfile")?,
        port: port.unwrap_or(445),
        smb302: version.unwrap_or(false),
    })
}

pub fn parse_credentials(text: &str, uid: u32, mode: u32) -> Result<Credentials, &'static str> {
    if uid != 0 || mode & 0o7777 != 0o600 {
        return Err("credential file must be root-owned and mode 0600");
    }
    let mut username = None;
    let mut password = None;
    let mut domain = None;
    for line in text.lines() {
        let (key, value) = line.split_once('=').ok_or("invalid credential file")?;
        match key {
            "username" if username.is_none() && !value.is_empty() => {
                username = Some(value.to_owned())
            }
            "password" if password.is_none() => password = Some(value.to_owned()),
            "domain" if domain.is_none() => domain = Some(value.to_owned()),
            _ => return Err("invalid credential file"),
        }
    }
    Ok(Credentials {
        username: username.ok_or("invalid credential file")?,
        password: password.ok_or("invalid credential file")?,
        domain: domain.unwrap_or_default(),
    })
}

pub fn read_credentials(path: &str) -> Result<Credentials, &'static str> {
    let mut file = std::fs::File::open(path).map_err(|_| "cannot open credential file")?;
    let metadata = file.metadata().map_err(|_| "cannot stat credential file")?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o7777 != 0o600 {
        return Err("credential file must be root-owned and mode 0600");
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|_| "cannot read credential file")?;
    parse_credentials(&text, metadata.uid(), metadata.mode())
}
