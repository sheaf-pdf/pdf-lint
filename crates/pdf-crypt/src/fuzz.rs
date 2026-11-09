//! The fuzz target `crypt`'s bodies, for the fuzz crate. They're compiled
//! only for tests and under cargo-fuzz's `--cfg fuzzing`.

#[cfg(any(test, feature = "insecure-test-writer"))]
use std::convert::Infallible;

/// The fuzz target `crypt`'s body: a dictionary of any version, revision
/// and fields of any length that `data` chooses, or one the writer makes
/// that opens, now and then another handler's or one without a `/Filter`,
/// opened with passwords from it, each encoding of its text and its bytes
/// as they are, and when it opens, strings and streams from it decrypted
/// by each method, one that isn't among them, and encrypted under IVs
/// from the operating system, and with the test writer under the IVs it
/// derives; and with the test writer, now and then, as revision 6 is, a new
/// file's dictionary created for passwords and permissions from it
/// (`created`).
/// Panics on a broken promise: none should; a dictionary of another handler
/// is refused as that, by its name, and none other is; a version refused
/// is the dictionary's; the handler says which password opened it, which
/// opens it alike alone, and the permissions its `/P` grants by its
/// revision; decrypting gives no more than it's given, and an error's
/// `Debug` gives what decrypted by its length; and what's encrypted
/// decrypts to what it was, AES's another IV each time from the operating
/// system and the same bytes each time from the derived IVs, or is refused
/// alike both ways.
pub fn handler(data: &[u8]) {
    use crate::{
        CryptFilter, Dictionary, Encoded, Fault, Handler, Method, Password, Permissions, Refused,
    };
    let mut bytes = data.iter().copied();
    let mut next = || bytes.next().unwrap_or(0);
    let mut take = |n: usize| -> Vec<u8> { (0..n).map(|_| next()).collect() };
    let head = take(8);
    let field = |at: usize| usize::from(head[at] % 64);
    let methods = ["V2", "AESV2", "AESV3", "None", "Odd"];
    // A field of the length `head[at]` chooses, or past 60, none at all.
    let mut string = |at: usize| {
        let value = take(field(at));
        (field(at) <= 60).then_some(value)
    };
    let dictionary = Dictionary {
        filter: b"Standard".to_vec(),
        v: i64::from(head[0] % 6),
        // Revision 6 runs thousands of AES blocks a password: rare.
        r: i64::from(head[1] % 7).min(if head[1] == 255 { 6 } else { 5 }),
        length: Some(i64::from(head[2]) * 2),
        o: string(3),
        u: string(4),
        oe: string(5),
        ue: string(6),
        perms: string(7),
        p: (head[5] != 0x7f).then_some(i64::from(i32::from_le_bytes([
            head[2], head[3], head[4], head[5],
        ]))),
        encrypt_metadata: Some(head[6] & 1 == 0),
        crypt_filters: vec![CryptFilter {
            name: b"StdCF".to_vec(),
            method: Some(
                methods[usize::from(head[7]) % methods.len()]
                    .as_bytes()
                    .to_vec(),
            ),
        }],
        stm_f: Some(b"StdCF".to_vec()),
        str_f: Some(b"StdCF".to_vec()),
    };
    let id = take(usize::from(head[0] % 20));
    let password = take(usize::from(head[1] % 8));
    // A byte of the 28 left for the `/ID` and the password, which take 26
    // at most, so `rest` starts where it did: now and then, another
    // handler's name, or none, as a reader leaves `/Filter` empty where
    // it's missing or not a name.
    let filter = match take(1)[0] % 32 {
        1 => Some(Vec::new()),
        2 => Some(b"Adobe.PubSec".to_vec()),
        3 => Some(id.clone()),
        _ => None,
    };
    // Half the time, a dictionary the writer makes for the password, which
    // opens, so that decrypting is fuzzed too.
    #[cfg(any(test, feature = "insecure-test-writer"))]
    let dictionary = match head[0] & 0x80 {
        0 => dictionary,
        _ => {
            use crate::writer::{Kind, Settings, dictionary};
            let kinds = [Kind::Rc4By40, Kind::Rc4By128, Kind::Aes128, Kind::Aes256R5];
            let kind = match head[1] {
                255 => Kind::Aes256,
                n => kinds[usize::from(n) % kinds.len()],
            };
            dictionary(&Settings {
                kind,
                user: &password,
                owner: b"owner",
                p: i64::from(i32::from_le_bytes([head[2], head[3], head[4], head[5]])),
                id: &id,
                encrypt_metadata: head[6] & 1 == 0,
            })
        }
    };
    // Whichever made it, the writer's among them, so that one which would
    // open is refused for its handler alone.
    let dictionary = match filter {
        Some(filter) => Dictionary {
            filter,
            ..dictionary
        },
        None => dictionary,
    };
    let rest: Vec<u8> = data.iter().copied().skip(8 + 5 * 64 + 28).collect();
    // A new file's dictionary runs revision 6's hash a dozen times: rare.
    #[cfg(any(test, feature = "insecure-test-writer"))]
    if head[1] == 254 {
        created(&rest, head[6] & 1 == 0);
    }
    let text = String::from_utf8_lossy(&password);
    let mut passwords = Password(&text).encodings();
    passwords.extend([Encoded::verbatim(&password), Encoded::verbatim(b"")]);
    let opened = Handler::open(&dictionary, &id, &passwords);
    let other = !dictionary.filter.is_empty() && dictionary.filter != b"Standard";
    match &opened {
        Err(Refused::OtherHandler { name }) => {
            assert!(
                other && *name == dictionary.filter,
                "another handler's, by its name"
            );
        }
        _ => assert!(!other, "another handler's refused as that"),
    }
    if let Err(Refused::Version { v, r }) = &opened {
        assert_eq!(
            (*v, *r),
            (dictionary.v, dictionary.r),
            "the version refused"
        );
    }
    let Ok(handler) = opened else {
        return;
    };
    let (i, encoding) = handler.opened_by().expect("a password given opened it");
    assert_eq!(passwords[i].encoding(), encoding, "the password's encoding");
    let alone = Handler::open(&dictionary, &id, &passwords[i..=i]).expect("it opens alone");
    assert_eq!(alone.opened_by(), Some((0, encoding)), "opened by it alone");
    assert_eq!(
        (alone.owner(), alone.faults()),
        (handler.owner(), handler.faults()),
        "opened alike alone"
    );
    let p = dictionary.p.expect("/P, as it opened");
    let permissions = Permissions::from_p(p, dictionary.r);
    assert_eq!(handler.permissions(), permissions, "/P's permissions");
    let id = (u32::from(head[2]), u16::from(head[3]));
    let methods = [
        None,
        Some(Method::Identity),
        Some(Method::None),
        Some(Method::Rc4),
        Some(Method::Aes128),
        Some(Method::Aes256),
        Some(Method::Unknown(password.clone())),
    ];
    for method in &methods {
        for data in [
            &rest[..],
            &rest[..rest.len().min(17)],
            &rest[..rest.len() / 16 * 16],
        ] {
            let check = |result: Result<Vec<u8>, crate::Error>| {
                let len = match result {
                    Ok(out) => out.len(),
                    Err(e) => {
                        by_length(&e);
                        e.data.map_or(0, |d| d.len())
                    }
                };
                assert!(len <= data.len(), "no more out than in");
            };
            check(handler.decrypt_stream(id, data, method.as_ref()));
            check(handler.decrypt_string(id, data));
        }
    }
    // Encrypted under IVs from the operating system, by each method:
    // decrypted, it's what it was, AES's IV another each time; or it's
    // refused, as decrypting refuses it. With the test writer, under the
    // derived IVs, the same bytes each time.
    let ivs = crate::Ivs::new();
    for method in &methods {
        for data in [&rest[..], &rest[..rest.len().min(17)]] {
            let stream = handler.encrypt_stream(id, data, method.as_ref(), &ivs);
            let again = handler.encrypt_stream(id, data, method.as_ref(), &ivs);
            let aes = matches!(
                method.as_ref().unwrap_or(handler.streams()),
                Method::Aes128 | Method::Aes256
            );
            match (&stream, &again) {
                (Ok(stream), Ok(again)) if aes => {
                    assert_ne!(stream[..16], again[..16], "another IV each time");
                }
                _ => assert_eq!(stream, again, "the same, or refused alike"),
            }
            #[cfg(any(test, feature = "insecure-test-writer"))]
            {
                let derived = crate::Ivs::derived();
                let once = handler.encrypt_stream(id, data, method.as_ref(), &derived);
                let twice = handler.encrypt_stream(id, data, method.as_ref(), &derived);
                assert_eq!(once, twice, "the same each time, derived");
                assert_eq!(once.is_ok(), stream.is_ok(), "refused alike, derived");
            }
            let string = handler.encrypt_string(id, data, &ivs);
            let decrypted = [
                stream.map(|s| handler.decrypt_stream(id, &s, method.as_ref())),
                string.map(|s| handler.decrypt_string(id, &s)),
            ];
            for (decrypted, stream) in decrypted.into_iter().zip([true, false]) {
                match decrypted {
                    Ok(back) => assert_eq!(back.as_deref(), Ok(data), "decrypts to what it was"),
                    Err(fault) => {
                        assert!(matches!(fault, Fault::Key { .. } | Fault::Method { .. }));
                        // Ciphertext of no white-space, so a stream's isn't
                        // taken for it.
                        let refused = match stream {
                            true => handler.decrypt_stream(id, &[1; 32], method.as_ref()),
                            false => handler.decrypt_string(id, &[1; 32]),
                        };
                        if let Err(e) = &refused {
                            by_length(e);
                        }
                        assert_eq!(refused.map_err(|e| e.fault), Err(fault), "refused alike");
                    }
                }
            }
        }
    }
}

/// Panics unless `error`'s `Debug` gives what decrypted by its length,
/// never its bytes, beside its fault.
fn by_length(error: &crate::Error) {
    let data = match &error.data {
        Some(data) => format!("Some({} bytes)", data.len()),
        None => "None".to_string(),
    };
    let said = format!("Error {{ data: {data}, fault: {:?} }}", error.fault);
    assert_eq!(format!("{error:?}"), said, "an error's Debug");
}

/// A new file's handler and dictionary (the test writer's
/// [`create`](crate::writer::create), from a generator, beside
/// [`Handler::create`](crate::Handler::create) from the operating system),
/// protected as `data` says: a user's text, then an owner's, each by its
/// length's byte and the text (UTF-8, lossily), none where the byte is 255
/// or there's none; then a byte whose bits choose the permissions
/// ([`permissions`]), every one where there's none; then what's drawn
/// ([`Drawn`]). Panics on a broken promise: none should; each of each
/// text's encodings, beside the other's prepared form, is taken exactly
/// when each password given is tagged SASLprep's and its bytes are a
/// password prepared (the text's, where it prepares, or 3.2's form of it
/// that prepares to itself), and the protection is one a file is made for,
/// else refused for the first password's tag, length or form, the user's
/// before the owner's; then, neither given, as no password; then, permissions
/// withheld, bit 10 put back, with no owner's password apart from the
/// user's, as shared. Made, the same draws give the same dictionary, of
/// revision 6, `/P` the permissions with bit 10, which opens by the owner's
/// text as the owner's, by the user's as the user's where the owner's is
/// apart and as the owner's where it isn't, and with no password, or the
/// empty one, as the user's where there's no user's password; and by no
/// other. `/Perms` confirms the key and matches `/P` and
/// `/EncryptMetadata` as asked, each handler says the permissions written,
/// and the handler made and each opened encrypt alike and each decrypts
/// the other's. `Handler::create` from the operating system refuses or
/// makes it alike, and what it makes opens by the same passwords.
#[cfg(any(test, feature = "insecure-test-writer"))]
fn created(data: &[u8], encrypt_metadata: bool) {
    use crate::writer::create;
    use crate::{Encoded, Encoding, Handler, Password, Permissions, Protection, Refused, Unusable};
    let (user, rest) = text(data);
    let (owner, rest) = text(rest);
    let (&bits, draws) = rest.split_first().unwrap_or((&0xff, &[]));
    let permissions = permissions(bits);
    let user = user.as_deref().map(Password);
    let owner = owner.as_deref().map(Password);
    // Its bytes, as text, a password that prepares to them.
    let itself = |bytes: &[u8]| {
        std::str::from_utf8(bytes).is_ok_and(|text| {
            Password(text)
                .prepared()
                .is_ok_and(|p| p.as_bytes() == bytes)
        })
    };
    // Why a password given can't be a new file's, if it can't.
    let unusable = |form: &Encoded| match (form.encoding(), form.as_bytes()) {
        (Encoding::Saslprep, []) => Some(Unusable::Empty),
        (Encoding::Saslprep, bytes) if bytes.len() > 127 => {
            Some(Unusable::TooLong { len: bytes.len() })
        }
        (Encoding::Saslprep, bytes) if itself(bytes) => None,
        (encoding, _) => Some(Unusable::Unprepared(encoding)),
    };
    // Whether the owner's password is apart from the user's.
    let apart = |user: Option<&Encoded>, owner: Option<&Encoded>| match (user, owner) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(user), Some(owner)) => user.as_bytes() != owner.as_bytes(),
    };
    let written = permissions | Permissions::ACCESSIBILITY;
    let expected = |protection: &Protection| {
        let passwords = [protection.user, protection.owner];
        if let Some(why) = passwords.into_iter().flatten().find_map(unusable) {
            return Err(why);
        }
        if passwords.iter().all(Option::is_none) {
            return Err(Unusable::NoPassword);
        }
        match written == Permissions::ALL || apart(protection.user, protection.owner) {
            true => Ok(()),
            false => Err(Unusable::Shared),
        }
    };
    fn protected<'a>(
        user: Option<&'a Encoded>,
        owner: Option<&'a Encoded>,
        (permissions, encrypt_metadata): (Permissions, bool),
    ) -> Protection<'a> {
        let mut protection = Protection::DEFAULT;
        protection.user = user;
        protection.owner = owner;
        protection.permissions = permissions;
        protection.encrypt_metadata = encrypt_metadata;
        protection
    }
    let how = (permissions, encrypt_metadata);
    let user_prepared = user.map(|typed| typed.prepared());
    let owner_prepared = owner.map(|typed| typed.prepared());
    let user_ok = user_prepared.as_ref().and_then(|p| p.as_ref().ok());
    let owner_ok = owner_prepared.as_ref().and_then(|p| p.as_ref().ok());
    // Each form of each text, the other's prepared form beside it.
    let forms = |typed: Option<Password>| typed.map(|t| t.encodings()).unwrap_or_default();
    let users = forms(user).into_iter().map(|form| (form, true));
    let owners = forms(owner).into_iter().map(|form| (form, false));
    for (form, is_user) in users.chain(owners) {
        let protection = match is_user {
            true => protected(Some(&form), owner_ok, how),
            false => protected(user_ok, Some(&form), how),
        };
        let created = create(&protection, &mut Drawn::over(draws));
        let created = created.map(|_| ());
        assert_eq!(created, expected(&protection), "refused for the first why");
        if created.is_ok() {
            let prepared = [&user_prepared, &owner_prepared][usize::from(!is_user)];
            if let Some(Ok(prepared)) = prepared {
                assert_eq!(
                    prepared.as_bytes(),
                    form.as_bytes(),
                    "the text's, where it prepares"
                );
            }
        }
    }
    let neither = create(&protected(None, None, how), &mut Drawn::over(draws));
    assert_eq!(neither.map(|_| ()), Err(Unusable::NoPassword), "neither");
    // A text given that doesn't prepare makes no file.
    let unprepared =
        |prepared: &Option<Result<Encoded, Unusable>>| matches!(prepared, Some(Err(_)));
    if unprepared(&user_prepared) || unprepared(&owner_prepared) {
        return;
    }
    let protection = protected(user_ok, owner_ok, how);
    let created = create(&protection, &mut Drawn::over(draws));
    let expected = expected(&protection);
    let from_os = Handler::create(&protection);
    assert_eq!(
        from_os.as_ref().map(|_| ()),
        created.as_ref().map(|_| ()),
        "made or refused alike from the operating system"
    );
    let Ok((handler, dictionary)) = created else {
        assert_eq!(created.map(|_| ()), expected, "refused, prepared");
        return;
    };
    assert_eq!(expected, Ok(()), "made, prepared");
    let again = create(&protection, &mut Drawn::over(draws));
    assert_eq!(
        again.map(|(_, d)| d).as_ref(),
        Ok(&dictionary),
        "the same draws"
    );
    assert_eq!(
        (dictionary.v, dictionary.r, dictionary.p),
        (5, 6, Some(written.p())),
        "revision 6, the permissions with bit 10"
    );
    assert!(handler.owner() && handler.opened_by().is_none() && handler.faults().is_empty());
    assert_eq!(handler.permissions(), written, "the permissions written");
    // Who opens it, and as whose.
    let mut opens = Vec::new();
    if let Some(owner) = owner {
        opens.push((owner.encodings(), true));
    }
    match user {
        Some(user) => opens.push((user.encodings(), !apart(user_ok, owner_ok))),
        None => {
            opens.push((Vec::new(), false));
            opens.push((vec![Encoded::verbatim(b"")], false));
        }
    }
    let id = (1, 0);
    let (_, os) = from_os.expect("made from the operating system as from the generator");
    for (passwords, owner) in opens {
        let by_os = Handler::open(&os, b"", &passwords).expect("it opens");
        assert_eq!(by_os.owner(), owner, "as the owner's or the user's");
        let opened = Handler::open(&dictionary, b"", &passwords).expect("it opens");
        assert_eq!(opened.owner(), owner, "as the owner's or the user's");
        assert!(opened.faults().is_empty(), "/Perms confirms and matches");
        assert_eq!(opened.permissions(), written, "the permissions written");
        assert_eq!(
            opened.encrypts_metadata(),
            encrypt_metadata,
            "/EncryptMetadata"
        );
        let derived = crate::Ivs::derived();
        for data in [draws, &draws[..draws.len().min(17)]] {
            let string = handler.encrypt_string(id, data, &derived);
            assert_eq!(opened.encrypt_string(id, data, &derived), string, "alike");
            let string = string.expect("AES-256");
            assert_eq!(opened.decrypt_string(id, &string), Ok(data.to_vec()));
            let stream = opened
                .encrypt_stream(id, data, None, &crate::Ivs::new())
                .expect("AES-256");
            assert_eq!(handler.decrypt_stream(id, &stream, None), Ok(data.to_vec()));
        }
    }
    // No other: bytes no prepared password is, which a file with no user's
    // password doesn't take for the empty one.
    let wrong = Handler::open(&dictionary, b"", &[Encoded::verbatim(b"\xff")]);
    assert_eq!(wrong.unwrap_err(), Refused::Password, "a wrong password");
}

/// A text from `data`: its first byte its length, then the text (UTF-8,
/// lossily), none where the byte is 255 or there's none; and what's after
/// it.
#[cfg(any(test, feature = "insecure-test-writer"))]
fn text(data: &[u8]) -> (Option<String>, &[u8]) {
    match data.split_first() {
        None => (None, data),
        Some((255, rest)) => (None, rest),
        Some((&len, rest)) => {
            let (text, rest) = rest.split_at(usize::from(len).min(rest.len()));
            (Some(String::from_utf8_lossy(text).into_owned()), rest)
        }
    }
}

/// The permissions whose bits are set in `bits`, the lowest the first
/// in the order of `/P`'s bits: printing, modifying, copying, annotating,
/// filling in forms, accessibility, assembling, printing at high
/// resolution. Each of the 256 sets, by one byte; the tests take them so
/// too.
#[cfg(any(test, feature = "insecure-test-writer"))]
pub(crate) fn permissions(bits: u8) -> crate::Permissions {
    use crate::Permissions;
    let each = [
        Permissions::PRINT,
        Permissions::MODIFY,
        Permissions::COPY,
        Permissions::ANNOTATE,
        Permissions::FILL_FORMS,
        Permissions::ACCESSIBILITY,
        Permissions::ASSEMBLE,
        Permissions::PRINT_HIGH,
    ];
    (0..8)
        .filter(|i| bits >> i & 1 == 1)
        .fold(Permissions::NONE, |set, i| set | each[i])
}

/// A random number generator for the fuzz body alone: the input's bytes,
/// then a counter's. It's no cryptographic generator, but says it is
/// (`TryCryptoRng`) so that the test writer's
/// [`create`](crate::writer::create) takes it, which only a fuzz body may
/// do: it's compiled for nothing else, and only with the test writer.
#[cfg(any(test, feature = "insecure-test-writer"))]
struct Drawn<'a> {
    data: &'a [u8],
    counter: u8,
}

#[cfg(any(test, feature = "insecure-test-writer"))]
impl Drawn<'_> {
    fn over(data: &[u8]) -> Drawn<'_> {
        Drawn { data, counter: 0 }
    }
}

#[cfg(any(test, feature = "insecure-test-writer"))]
impl rand_core::TryRng for Drawn<'_> {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut bytes = [0; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut bytes = [0; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        for byte in dst {
            *byte = match self.data.split_first() {
                Some((&first, rest)) => {
                    self.data = rest;
                    first
                }
                None => {
                    self.counter = self.counter.wrapping_add(1);
                    self.counter
                }
            };
        }
        Ok(())
    }
}

#[cfg(any(test, feature = "insecure-test-writer"))]
impl rand_core::TryCryptoRng for Drawn<'_> {}

/// The fuzz target `crypt`'s body for passwords as typed: text from
/// `data`, read as UTF-8 (lossily), as a character each two bytes (the
/// Basic Multilingual Plane's) and as one each three (every plane's),
/// given to `Password::encodings` and `Password::prepared`. Panics on a
/// broken promise: none should; the forms are SASLprep's by Unicode 3.2
/// first where it isn't the text, then SASLprep's by a later Unicode
/// (stringprep's) where it's neither, then the text as typed, then
/// single-byte forms of the text composed, a byte a character, none twice
/// among those a revision takes first; 3.2's form is there exactly where
/// stringprep takes the text with Corrigendum #4's five characters mapped to
/// their 3.2 decompositions and 3.2 assigned each of its characters, and has
/// that form's characters, decomposed, in another order at most; prepared, the
/// text is SASLprep's form by both, of 1 to 127 bytes and its own SASLprep by
/// both, or it's refused for what the text is: over `Password::LONGEST` bytes
/// as typed, whatever else it is; a character by its place and the first table
/// it's in; forms that part at a place no shorter beginning of the text parts
/// at.
pub fn password(data: &[u8]) {
    use crate::password::{CORRIGENDUM_4, saslprep_32};
    use crate::{Encoding, Password, Unusable};
    use stringprep::saslprep;
    use stringprep::tables::unassigned_code_point;
    use unicode_normalization::UnicodeNormalization;
    let lossy = String::from_utf8_lossy(data);
    let wide: String = (data.as_chunks::<2>().0.iter())
        .filter_map(|&b| char::from_u32(u32::from(u16::from_le_bytes(b))))
        .collect();
    let planes: String = (data.as_chunks::<3>().0.iter())
        .filter_map(|&b| char::from_u32(u32::from_le_bytes([b[0], b[1], b[2], 0]) % 0x11_0000))
        .collect();
    // Text decomposed, its characters in order.
    let sorted = |text: &str| {
        let mut chars: Vec<char> = text.nfd().collect();
        chars.sort_unstable();
        chars
    };
    for text in [&lossy[..], &wide, &planes] {
        let typed = Password(text);
        let forms = typed.encodings();
        let form = |i: usize| (forms[i].encoding(), forms[i].as_bytes());
        let prepared = saslprep(text);
        let unicode_32 = saslprep_32(text);
        let mut first = 0;
        if let Some(old) = unicode_32
            .as_ref()
            .filter(|old| old[..] != *text.as_bytes())
        {
            assert_eq!(form(0), (Encoding::Saslprep, &old[..]), "SASLprep's by 3.2");
            first = 1;
        }
        if let Ok(new) = &prepared {
            let other = unicode_32
                .as_ref()
                .is_none_or(|old| old[..] != *new.as_bytes());
            if new != text && other {
                let new = (Encoding::SaslprepCurrentUnicode, new.as_bytes());
                assert_eq!(form(first), new, "SASLprep's by a later Unicode");
                first += 1;
            }
        }
        assert_eq!(form(first), (Encoding::Utf8, text.as_bytes()), "as typed");
        let mapped: String = (text.chars())
            .map(|c| (CORRIGENDUM_4.iter().find(|&&(x, _)| x == c)).map_or(c, |&(_, old)| old))
            .collect();
        let of_mapped = saslprep(&mapped);
        let assigned = !text.chars().any(unassigned_code_point);
        assert_eq!(
            unicode_32.is_some(),
            of_mapped.is_ok() && assigned,
            "3.2's where SASLprep takes it"
        );
        if let (Some(old), Ok(of_mapped)) = (&unicode_32, &of_mapped) {
            let old = std::str::from_utf8(old).expect("UTF-8");
            assert_eq!(sorted(old), sorted(of_mapped), "3.2's characters");
        }
        let len = text.nfc().count();
        for (encoding, bytes) in (first + 1..forms.len()).map(form) {
            let single = [Encoding::PdfDoc, Encoding::WinAnsi, Encoding::MacRoman];
            assert!(single.contains(&encoding), "{encoding:?}");
            assert_eq!(bytes.len(), len, "a byte a character composed");
        }
        for revision in 2..=6 {
            let fit: Vec<&[u8]> = (forms.iter())
                .filter(|f| f.encoding().fits(revision))
                .map(|f| f.as_bytes())
                .collect();
            for (i, a) in fit.iter().enumerate() {
                assert!(!fit[i + 1..].contains(a), "twice for revision {revision}");
            }
        }
        let capped = text.len() <= Password::LONGEST;
        match typed.prepared() {
            Ok(encoded) => {
                assert!(capped, "prepared within the cap");
                assert_eq!(encoded.encoding(), Encoding::Saslprep);
                let bytes = encoded.as_bytes();
                assert!((1..=127).contains(&bytes.len()), "1 to 127 bytes");
                assert_eq!(
                    unicode_32.as_ref().map(|old| &old[..]),
                    Some(bytes),
                    "3.2's"
                );
                let encoded = std::str::from_utf8(bytes).expect("UTF-8");
                assert_eq!(prepared.as_deref().ok(), Some(encoded), "SASLprep's");
                let again = saslprep(encoded);
                assert_eq!(again.as_deref().ok(), Some(encoded), "its own SASLprep");
                let again = saslprep_32(encoded);
                assert_eq!(
                    again.as_ref().map(|old| &old[..]),
                    Some(bytes),
                    "its own by 3.2"
                );
            }
            Err(Unusable::Empty) => assert!(text.is_empty(), "empty"),
            Err(Unusable::Vanished) => {
                assert!(
                    !text.is_empty() && prepared.as_deref().ok() == Some(""),
                    "vanished"
                );
            }
            Err(Unusable::TooLong { len }) => {
                assert!(len > 127, "too long");
                assert_eq!(prepared.map(|p| p.len()).ok(), Some(len), "its length");
            }
            Err(Unusable::TypedTooLong { len, most }) => {
                assert_eq!(len, text.len(), "its length as typed");
                assert_eq!(most, Password::LONGEST, "the most");
                assert!(len > most, "too long as typed");
            }
            Err(Unusable::Prohibited { at, why }) => {
                let c = text.chars().nth(at).expect("a character of the text");
                // Unassigned in 3.2, or refused, alone too.
                let alone = saslprep(c.encode_utf8(&mut [0; 4])).is_err();
                assert!(unassigned_code_point(c) || (prepared.is_err() && alone));
                // The first of its tables.
                let of = crate::password::Prohibition::of(c);
                assert_eq!(of, Some(why), "its table");
            }
            Err(Unusable::Bidi) => assert!(prepared.is_err(), "bidi"),
            Err(Unusable::Ambiguous { at }) => {
                assert!(at < text.chars().count(), "ambiguous at a character");
                let new = prepared.as_deref().map(str::as_bytes).ok();
                assert!(new.is_some_and(|new| (1..=127).contains(&new.len())));
                assert_ne!(
                    unicode_32.as_ref().map(|old| &old[..]),
                    new,
                    "the forms differ"
                );
                // No beginning of it before the place has forms that differ.
                let end = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
                for n in 1..=at {
                    let begun = &text[..end(n)];
                    let begun_new = saslprep(begun).ok().map(|f| f.into_owned().into_bytes());
                    let begun_old = saslprep_32(begun).map(|f| f.to_vec());
                    assert_eq!(begun_old, begun_new, "parted before its place");
                }
            }
            Err(other) => panic!("{other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{handler, password};

    #[test]
    fn random_dictionaries_and_ciphertext_keep_every_promise() {
        let mut seed = 0x853c_49e6_748f_ea9b_u64;
        for i in 0..400 {
            let data: Vec<u8> = (0..300 + i)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as u8
                })
                .collect();
            handler(&data);
        }
    }

    /// New files' dictionaries, as the fuzz body creates them now and then:
    /// for text whose SASLprep form is itself, isn't, is empty, is over 127
    /// bytes, or that SASLprep refuses; whose forms by Unicode 3.2 and by a
    /// later Unicode differ, Corrigendum #4's way and #5's; and for random
    /// text. Each as the user's password alone with every permission, with
    /// draws and with none but the counter's, each way of
    /// `/EncryptMetadata`; and with draws, each way of it by turns, as the
    /// user's alone with permissions withheld (none, all but bit 10, which
    /// is put back, and some), beside the next text as the owner's and
    /// beside itself, and as the owner's alone with every permission and
    /// none; and neither.
    #[test]
    fn new_files_dictionaries_keep_every_promise() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        };
        let long = "\u{2168}".repeat(64);
        let random: Vec<u8> = (0..40).map(|_| next()).collect();
        let texts: [&[u8]; 11] = [
            b"user",
            "p\u{e4}ssw\u{f6}rd".as_bytes(),
            "\u{2168}".as_bytes(),
            "I\u{AD}X".as_bytes(),
            "\u{AD}".as_bytes(),
            long.as_bytes(),
            b"\x07",
            b"",
            "Password\u{2F874}!".as_bytes(),
            "\u{1100}\u{300}\u{1161}\u{323}".as_bytes(),
            &random,
        ];
        // A text's length and the text, or none.
        let text = |data: &mut Vec<u8>, text: Option<&[u8]>| match text {
            Some(text) => {
                data.push(text.len() as u8);
                data.extend_from_slice(text);
            }
            None => data.push(255),
        };
        let run = |user: Option<&[u8]>, owner: Option<&[u8]>, bits: u8, draws: &[u8], metadata| {
            let mut data = vec![0; 8 + 5 * 64 + 28];
            data[1] = 254;
            data[6] = metadata;
            text(&mut data, user);
            text(&mut data, owner);
            data.push(bits);
            data.extend_from_slice(draws);
            handler(&data);
        };
        for text in texts {
            for draws in [&[][..], &[7; 60]] {
                for metadata in [0, 1] {
                    run(Some(text), None, 0xff, draws, metadata);
                }
            }
        }
        let draws = [7; 60];
        for (i, text) in texts.into_iter().enumerate() {
            let metadata = (i % 2) as u8;
            let next = texts[(i + 1) % texts.len()];
            for bits in [0, 0xdf, 0x5a] {
                run(Some(text), None, bits, &draws, metadata);
            }
            run(Some(text), Some(next), 0x5a, &draws, metadata);
            run(Some(text), Some(text), 0x0f, &draws, metadata);
            for bits in [0xff, 0] {
                run(None, Some(text), bits, &draws, metadata);
            }
        }
        run(None, None, 0xff, &draws, 0);
    }

    #[test]
    fn random_text_keeps_every_promise_of_its_forms() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for i in 0..2000 {
            let len = (next() % 400) as usize;
            let data: Vec<u8> = (0..len)
                .map(|_| match i % 4 {
                    // Mostly ASCII, as typed text is; a byte past it now
                    // and then, which the second and third readings make
                    // characters of other scripts.
                    0 => b"aZ9 \xC3\xA9\x01\xADx"[(next() % 9) as usize],
                    _ => next() as u8,
                })
                .collect();
            password(&data);
        }
        for text in [
            "I\u{AD}X",
            "\u{2168}",
            "e\u{301}",
            "\u{5D0}\u{FB44}",
            "\u{AD}",
            // Corrigendum #4's, #3's, #5's, and a character 3.2 hadn't
            // assigned that a later Unicode decomposes.
            "Password\u{2F874}!",
            "\u{F951}",
            "\u{1100}\u{300}\u{1161}\u{323}",
            "\u{AC00}\u{300}\u{323}",
            "\u{B47}\u{300}\u{B3E}",
            "\u{2090}",
        ] {
            password(text.as_bytes());
        }
        password("\u{2168}".repeat(64).as_bytes());
    }
}
