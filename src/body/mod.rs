use core::fmt;
use std::io::{self, Read};

use crate::pool::Connection;
use crate::time::Instant;
use crate::unit::{Event, Input, Unit};
use crate::Error;

#[cfg(feature = "charset")]
mod charset;

pub struct Body {
    info: ResponseInfo,
    unit_handler: UnitHandler,
}

#[derive(Clone)]
pub(crate) struct ResponseInfo {
    content_encoding: ContentEncoding,
    mime_type: Option<String>,
    charset: Option<String>,
}

struct UnitHandler {
    unit: Unit<()>,
    connection: Option<Connection>,
    current_time: Box<dyn Fn() -> Instant + Send + Sync>,
}

impl Body {
    pub(crate) fn new(
        unit: Unit<()>,
        connection: Connection,
        info: ResponseInfo,
        current_time: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> Self {
        Body {
            info,
            unit_handler: UnitHandler {
                unit,
                connection: Some(connection),
                current_time: Box::new(current_time),
            },
        }
    }

    pub fn mime_type(&self) -> Option<&str> {
        self.info.mime_type.as_deref()
    }

    pub fn charset(&self) -> Option<&str> {
        self.info.charset.as_deref()
    }

    pub fn as_reader(&mut self, limit: u64) -> BodyReader {
        BodyReader::new(
            LimitReader::shared(&mut self.unit_handler, limit),
            &self.info,
        )
    }

    pub fn into_reader(self, limit: u64) -> BodyReader<'static> {
        BodyReader::new(LimitReader::owned(self.unit_handler, limit), &self.info)
    }

    pub fn read_to_string(&mut self, limit: usize) -> Result<String, Error> {
        let mut buf = String::new();
        let mut reader = self.as_reader(limit as u64);
        reader.read_to_string(&mut buf)?;
        Ok(buf)
    }

    pub fn read_to_vec(&mut self, limit: usize) -> Result<Vec<u8>, Error> {
        let mut buf = Vec::new();
        let mut reader = self.as_reader(limit as u64);
        reader.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

impl UnitHandler {
    fn do_read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let now = (self.current_time)();

        let Some(connection) = &mut self.connection else {
            return Ok(0);
        };

        let event = self.unit.poll_event((self.current_time)())?;

        let timeout = match event {
            Event::AwaitInput { timeout } => timeout,
            Event::Reset { must_close } => {
                if let Some(connection) = self.connection.take() {
                    if must_close {
                        connection.close()
                    } else {
                        connection.reuse(now)
                    }
                }
                return Ok(0);
            }
            _ => unreachable!("Expected event AwaitInput"),
        };

        connection.await_input(timeout)?;
        let input = connection.buffers().input();

        let max = input.len().min(buf.len());
        let input = &input[..max];

        let input_used =
            self.unit
                .handle_input((self.current_time)(), Input::Data { input }, buf)?;

        connection.consume_input(input_used);

        let event = self.unit.poll_event((self.current_time)())?;

        let Event::ResponseBody { amount } = event else {
            unreachable!("Expected event ResponseBody");
        };

        Ok(amount)
    }
}

#[derive(Debug, Clone, Copy)]
enum ContentEncoding {
    None,
    Gzip,
    Brotli,
    Unknown,
}

impl ResponseInfo {
    pub fn new(headers: &http::HeaderMap) -> Self {
        let content_encoding = headers
            .get("content-encoding")
            .and_then(|v| v.to_str().ok())
            .map(ContentEncoding::from)
            .unwrap_or(ContentEncoding::None);

        let (mime_type, charset) = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(split_content_type)
            .unwrap_or((None, None));

        ResponseInfo {
            content_encoding,
            mime_type,
            charset,
        }
    }
}

fn split_content_type(content_type: &str) -> (Option<String>, Option<String>) {
    // Content-Type: text/plain; charset=iso-8859-1
    let mut split = content_type.split(';');

    let Some(mime_type) = split.next() else {
        return (None, None);
    };

    let mut charset = None;

    for maybe_charset in split {
        let maybe_charset = maybe_charset.trim();
        if let Some(s) = maybe_charset.strip_prefix("charset=") {
            charset = Some(s.to_string());
        }
    }

    (Some(mime_type.to_string()), charset)
}

pub struct BodyReader<'a> {
    reader: CharsetDecoder<ContentDecoder<LimitReader<'a>>>,
}

impl<'a> BodyReader<'a> {
    fn new(reader: LimitReader<'a>, info: &ResponseInfo) -> BodyReader<'a> {
        let reader = content_decoder(reader, info.content_encoding);
        let reader = charset_decoder(reader, info.mime_type.as_deref(), info.charset.as_deref());
        BodyReader { reader }
    }
}

fn content_decoder<R: Read>(reader: R, content_encoding: ContentEncoding) -> ContentDecoder<R> {
    let decoder = match content_encoding {
        ContentEncoding::None => ContentDecoder::PassThrough(reader),
        #[cfg(feature = "gzip")]
        ContentEncoding::Gzip => ContentDecoder::Gzip(flate2::read::MultiGzDecoder::new(reader)),
        #[cfg(not(feature = "gzip"))]
        ContentEncoding::Gzip => {
            info!("Not decompressing. Enable feature gzip");
            ContentDecoder::Gzip(reader)
        }
        #[cfg(feature = "brotli")]
        ContentEncoding::Brotli => {
            ContentDecoder::Brotli(brotli_decompressor::Decompressor::new(reader, 4096))
        }
        #[cfg(not(feature = "brotli"))]
        ContentEncoding::Brotli => {
            info!("Not decompressing. Enable feature brotli");
            ContentDecoder::Brotli(reader)
        }
        ContentEncoding::Unknown => {
            info!("Unknown content-encoding");
            ContentDecoder::PassThrough(reader)
        }
    };

    debug!(
        "content_encoding {:?} resulted in decoder: {:?}",
        content_encoding, decoder
    );

    decoder
}

fn charset_decoder<R: Read>(
    reader: R,
    mime_type: Option<&str>,
    charset: Option<&str>,
) -> CharsetDecoder<R> {
    let is_text = mime_type.map(|m| m.starts_with("text/")).unwrap_or(false);

    let decoder = if is_text {
        #[cfg(feature = "charset")]
        {
            let from = charset
                .and_then(|c| encoding_rs::Encoding::for_label(c.as_bytes()))
                .unwrap_or(encoding_rs::UTF_8);

            if from == encoding_rs::UTF_8 {
                // Do nothing
                CharsetDecoder::PassThrough(reader)
            } else {
                CharsetDecoder::Decoder(self::charset::CharCodec::new(
                    reader,
                    from,
                    encoding_rs::UTF_8,
                ))
            }
        }
        #[cfg(not(feature = "charset"))]
        {
            CharsetDecoder::Decoder(reader)
        }
    } else {
        CharsetDecoder::PassThrough(reader)
    };

    debug!(
        "mime_type {:?} charset {:?} resulted in decoder: {:?}",
        mime_type, charset, decoder
    );

    decoder
}

impl<'a> Read for BodyReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

enum CharsetDecoder<R> {
    #[cfg(feature = "charset")]
    Decoder(self::charset::CharCodec<R>),
    #[cfg(not(feature = "charset"))]
    Decoder(R),
    PassThrough(R),
}

impl<R: io::Read> Read for CharsetDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            CharsetDecoder::Decoder(v) => v.read(buf),
            CharsetDecoder::PassThrough(v) => v.read(buf),
        }
    }
}

enum ContentDecoder<R: io::Read> {
    #[cfg(feature = "gzip")]
    Gzip(flate2::read::MultiGzDecoder<R>),
    #[cfg(not(feature = "gzip"))]
    Gzip(R),
    #[cfg(feature = "brotli")]
    Brotli(brotli_decompressor::Decompressor<R>),
    #[cfg(not(feature = "brotli"))]
    Brotli(R),
    PassThrough(R),
}

impl<R: Read> Read for ContentDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ContentDecoder::Gzip(v) => v.read(buf),
            ContentDecoder::Brotli(v) => v.read(buf),
            ContentDecoder::PassThrough(v) => v.read(buf),
        }
    }
}

struct LimitReader<'a> {
    unit_handler: UnitHandlerRef<'a>,
    left: u64,
}

enum UnitHandlerRef<'a> {
    Shared(&'a mut UnitHandler),
    Owned(UnitHandler),
}

impl<'a> UnitHandlerRef<'a> {
    fn do_read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        match self {
            UnitHandlerRef::Shared(v) => v.do_read(buf),
            UnitHandlerRef::Owned(v) => v.do_read(buf),
        }
    }
}

impl<'a> LimitReader<'a> {
    fn shared(u: &'a mut UnitHandler, limit: u64) -> LimitReader<'a> {
        Self {
            unit_handler: UnitHandlerRef::Shared(u),
            left: limit,
        }
    }
}

impl LimitReader<'static> {
    fn owned(u: UnitHandler, limit: u64) -> LimitReader<'static> {
        Self {
            unit_handler: UnitHandlerRef::Owned(u),
            left: limit,
        }
    }
}

impl<'a> Read for LimitReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(Error::BodyExceedsLimit.into_io());
        }

        // The max buffer size is usize, which may be 32 bit.
        let max = (self.left.min(usize::MAX as u64) as usize).min(buf.len());

        let n = self
            .unit_handler
            .do_read(&mut buf[..max])
            .map_err(|e| e.into_io())?;

        self.left -= n as u64;

        Ok(n)
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body").finish()
    }
}

impl From<&str> for ContentEncoding {
    fn from(s: &str) -> Self {
        match s {
            "gzip" => ContentEncoding::Gzip,
            "br" => ContentEncoding::Brotli,
            _ => {
                info!("Unknown content-encoding: {}", s);
                ContentEncoding::Unknown
            }
        }
    }
}

impl<R: Read> fmt::Debug for ContentDecoder<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gzip(_) => f
                .debug_tuple(
                    #[cfg(feature = "gzip")]
                    "Gzip",
                    #[cfg(not(feature = "gzip"))]
                    "Gzip(disabled)",
                )
                .finish(),
            Self::Brotli(_) => f
                .debug_tuple(
                    #[cfg(feature = "brotli")]
                    "Brotli",
                    #[cfg(not(feature = "brotli"))]
                    "Brotli(disabled)",
                )
                .finish(),
            Self::PassThrough(_) => f.debug_tuple("PassThrough").finish(),
        }
    }
}

impl<R> fmt::Debug for CharsetDecoder<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decoder(_) => f
                .debug_struct(
                    #[cfg(feature = "charset")]
                    "Decoder",
                    #[cfg(not(feature = "charset"))]
                    "Decoder(disabled)",
                )
                .finish(),
            Self::PassThrough(_) => f.debug_tuple("PassThrough").finish(),
        }
    }
}
