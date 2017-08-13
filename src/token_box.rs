use mio;
use std::usize;
use types::*;

#[derive(Debug)]
pub enum TypedToken {
    Listener,
    Inotify,
    NurseryToken(ClientId),
    PoolToken(ClientId),
}

const CATEGORIES: usize = 2;

// TODO: Unit tests
pub fn from_token(token: mio::Token) -> TypedToken {
    let mio::Token(x) = token;
    let tt = if x == usize::MAX - 1 {
        TypedToken::Listener
    } else if x == usize::MAX - 2 {
        TypedToken::Inotify
    } else if x % CATEGORIES == 0 {
        TypedToken::NurseryToken(x / CATEGORIES)
    } else {
        TypedToken::PoolToken((x - 1) / CATEGORIES)
    };
    debug!("{:?} => {:?}", token, tt);
    tt
}

// TODO: Unit tests
pub fn to_token(tt: TypedToken) -> mio::Token {
    let token = mio::Token(match tt {
        TypedToken::Listener => usize::MAX - 1,
        TypedToken::Inotify => usize::MAX - 2,
        TypedToken::NurseryToken(x) => x * CATEGORIES,
        TypedToken::PoolToken(x) => x * CATEGORIES + 1,
    });
    debug!("{:?} => {:?}", tt, token);
    token
}
