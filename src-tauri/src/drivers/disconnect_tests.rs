use crate::error::{AppError, DisconnectPhase};

fn mysql_code(code: u16) -> AppError {
    super::mysql::connection_error(mysql::Error::MySqlError(mysql::MySqlError {
        state: "HY000".into(),
        message: "server fixture".into(),
        code,
    }))
}

pub(crate) fn lost_response_fixtures() -> Vec<AppError> {
    use std::io::{Error, ErrorKind};
    let dropped = || Error::from(ErrorKind::ConnectionReset);
    vec![
        mysql_code(4031),
        super::mongo::connection_error(mongodb::error::ErrorKind::Io(dropped().into()).into()),
        super::redis::connection_error(dropped().into()),
        super::mssql::connection_error(dropped().into()),
        super::dameng::connection_error(dameng::Error::Io(dropped())),
        super::oracle::sqlplus_error("ORA-03113: end-of-file on communication channel"),
        super::snowflake::request_error("POST", dropped().into()),
        super::snowflake::request_error(
            "GET",
            ureq::Response::new(502, "Bad Gateway", "").unwrap().into(),
        ),
    ]
}

#[test]
fn protocol_errors_distinguish_lost_responses_from_sql_and_authentication_failures() {
    let phase = |e: AppError| e.disconnect_phase();
    for e in lost_response_fixtures() {
        assert_eq!(phase(e), Some(DisconnectPhase::OutcomeUnknown));
    }
    for e in [
        mysql_code(1064),
        mysql_code(1062),
        mysql_code(1205),
        mysql_code(1317),
        super::mongo::connection_error(mongodb::error::Error::custom("not a network error")),
        super::redis::connection_error((redis::ErrorKind::ResponseError, "WRONGTYPE").into()),
        super::mssql::connection_error(tiberius::error::Error::Conversion("bad value".into())),
        super::dameng::connection_error(dameng::Error::AuthFailed("denied".into())),
        super::oracle::sqlplus_error("ORA-00054: resource busy"),
        super::snowflake::request_error(
            "POST",
            ureq::Response::new(422, "Invalid SQL", "").unwrap().into(),
        ),
    ] {
        assert_eq!(phase(e), None);
    }
}
