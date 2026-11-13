pub struct Options {
    pub width:usize, pub height:usize, pub file:Option<String>, pub session:Option<(bool,[u8;4],u16,String)>,
}
impl Options {
    pub fn parse(args:impl Iterator<Item=String>)->Result<Self,&'static str> {
        let mut options=Self {width:800,height:424,file:None,session:None};
        let mut mode=None; let mut name=None; let mut args=args;
        let usage="usage: guskit [--canvas WxH] [--host PORT|ADDR:PORT | --join PORT|ADDR[:PORT]] [--name NAME] [FILE]";
        while let Some(arg)=args.next() {
            match arg.as_str() {
                "--canvas"=>{
                    let value=args.next().ok_or(usage)?;
                    let (w,h)=value.split_once('x').ok_or(usage)?;
                    options.width=w.parse().map_err(|_|usage)?;options.height=h.parse().map_err(|_|usage)?;
                    if options.width==0||options.width>800||options.height==0||options.height>424 {return Err(usage);}
                }
                "--host"|"--join"=>{
                    if mode.is_some(){return Err(usage);}
                    let value=args.next().ok_or(usage)?;
                    let (address, port) = if value.bytes().all(|b| b.is_ascii_digit()) {
                        let port=value.parse::<u16>().map_err(|_|usage)?;
                        if port==0{return Err(usage);}
                        ([127,0,0,1],port)
                    } else {
                        if arg=="--host" && !value.contains(':') {return Err(usage);}
                        libcollab::endpoint::parse_endpoint(&value).map_err(|_|usage)?
                    };
                    mode=Some((arg=="--host",address,port));
                }
                "--name"=>{if name.is_some(){return Err(usage);} let value=args.next().ok_or(usage)?;
                    if value.is_empty()||value.len()>64||value.chars().any(char::is_control){return Err(usage);}
                    name=Some(value);
                }
                _=>{if arg.starts_with('-')||options.file.is_some(){return Err(usage);} options.file=Some(arg);}
            }
        }
        options.session=match (mode,name) { (Some((host,address,port)),Some(name))=>Some((host,address,port,name)),(None,None)=>None,_=>return Err(usage) };
        Ok(options)
    }
}
