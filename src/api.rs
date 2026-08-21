//! 塔吉多 / 老虎 API 客户端，与上游 `taygedo/api.ts` 对应。

use std::collections::BTreeMap;

use reqwest::header::{HeaderMap, HeaderValue};

use crate::constants::*;
use crate::protocol::{
    aes_ecb_base64, form_encode_pairs, laohu_base_params, make_ds, make_nonce, now_millis,
    now_seconds, signed_body,
};

#[derive(Debug)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ApiError {}

pub type Result<T> = std::result::Result<T, ApiError>;

pub struct GameRoleRaw {
    pub role_id: String,
    pub role_name: Option<String>,
}

pub struct GameRecordCard {
    pub game_id: String,
    pub game_name: Option<String>,
    pub role_id: Option<String>,
    pub role_name: Option<String>,
}

pub struct CoinTask {
    pub code: String,
    pub complete_times: i64,
    pub limit_times: i64,
}

pub struct CoinState {
    pub today_coin: Option<f64>,
    pub limit_coin: Option<f64>,
}

pub struct CloudInfo {
    pub gave: f64,
    pub remained: Option<f64>,
}

pub struct RecommendPost {
    pub post_id: String,
    pub liked: Option<bool>,
}

pub struct Api {
    pub client: reqwest::Client,
}

impl Api {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("构建 HTTP 客户端失败");
        Self { client }
    }

    // ---- 登录相关 ----

    pub async fn send_captcha(&self, phone: &str, device_id: &str) -> Result<()> {
        let mut data = laohu_base_params(device_id, &now_seconds().to_string(), "versionCode");
        data.insert("areaCodeId".into(), "1".into());
        data.insert("cellphone".into(), phone.into());
        data.insert("type".into(), "16".into());
        let body = signed_body(data, LAOHU_SECRET, false);

        let (status, text) = self
            .post_form(
                &format!("{LAOHU_BASE_URL}/m/newApi/sendPhoneCaptchaWithOutLogin"),
                laohu_headers(),
                body,
            )
            .await?;
        let (code, msg, _) = parse_body(&text, "sendCaptcha", status)?;

        let sending_ok = status == 200 && code == 1 && msg.contains("短信正在发送");
        if !is_ok(status) || (code != 0 && !sending_ok) {
            return Err(api_error("sendCaptcha", status, code, &msg));
        }
        Ok(())
    }

    async fn check_captcha(&self, phone: &str, captcha: &str, device_id: &str) -> Result<()> {
        let mut data = laohu_base_params(device_id, &now_seconds().to_string(), "versionCode");
        data.insert("captcha".into(), captcha.into());
        data.insert("cellphone".into(), phone.into());
        let body = signed_body(data, LAOHU_SECRET, false);

        let (status, text) = self
            .post_form(
                &format!("{LAOHU_BASE_URL}/m/newApi/checkPhoneCaptchaWithOutLogin"),
                laohu_headers(),
                body,
            )
            .await?;
        let (code, msg, _) = parse_body(&text, "checkCaptcha", status)?;
        if !is_ok(status) || code != 0 {
            return Err(api_error("checkCaptcha", status, code, &msg));
        }
        Ok(())
    }

    pub async fn login_with_captcha(
        &self,
        phone: &str,
        captcha: &str,
        device_id: &str,
    ) -> Result<(String, String)> {
        self.check_captcha(phone, captcha, device_id).await?;

        let mut data = laohu_base_params(device_id, &now_millis().to_string(), "version");
        data.insert("areaCodeId".into(), "1".into());
        data.insert("captcha".into(), aes_ecb_base64(captcha, LAOHU_SECRET));
        data.insert("cellphone".into(), aes_ecb_base64(phone, LAOHU_SECRET));
        data.insert("type".into(), "16".into());
        let body = signed_body(data, LAOHU_SECRET, true);

        let (status, text) = self
            .post_form(
                &format!("{LAOHU_BASE_URL}/openApi/sms/new/login"),
                laohu_headers(),
                body,
            )
            .await?;
        let (code, msg, full) = parse_body(&text, "loginWithCaptcha", status)?;

        let result = full.get("result");
        let token = result.and_then(|r| r.get("token")).and_then(value_to_string);
        let user_id = result.and_then(|r| r.get("userId")).and_then(value_to_string);

        if !is_ok(status) || code != 0 || token.is_none() || user_id.is_none() {
            return Err(api_error("loginWithCaptcha", status, code, &msg));
        }
        Ok((token.unwrap(), user_id.unwrap()))
    }

    pub async fn login_with_password(
        &self,
        phone: &str,
        password: &str,
        device_id: &str,
    ) -> Result<(String, String)> {
        let mut data = laohu_base_params(device_id, &now_millis().to_string(), "version");
        data.insert("password".into(), aes_ecb_base64(password, LAOHU_SECRET));
        data.insert("username".into(), aes_ecb_base64(phone, LAOHU_SECRET));
        let body = signed_body(data, LAOHU_SECRET, true);

        let mut headers = laohu_headers();
        headers.insert("robot-auth-type", HeaderValue::from_static("2"));

        let (status, text) = self
            .post_form(&format!("{LAOHU_BASE_URL}/openApi/secureLogin"), headers, body)
            .await?;
        let (code, msg, full) = parse_body(&text, "loginWithPassword", status)?;

        let result = full.get("result");
        let token = result.and_then(|r| r.get("token")).and_then(value_to_string);
        let user_id = result.and_then(|r| r.get("userId")).and_then(value_to_string);

        if !is_ok(status) || code != 0 || token.is_none() || user_id.is_none() {
            return Err(api_error("loginWithPassword", status, code, &msg));
        }
        Ok((token.unwrap(), user_id.unwrap()))
    }

    pub async fn user_center_login(
        &self,
        token: &str,
        user_id: &str,
        device_id: &str,
    ) -> Result<(String, String, String)> {
        let mut attempt = self
            .request_user_center_login(token, user_id, device_id, "official")
            .await;
        if let Ok((status, code, msg, _)) = &attempt {
            if *status == 200 && *code == 1 && msg.trim() == "系统错误" {
                let compat = self
                    .request_user_center_login(token, user_id, device_id, "compat-1.1.0")
                    .await;
                if let Ok((s, c, _, _)) = &compat {
                    if *s == 200 && *c == 0 {
                        attempt = compat;
                    }
                }
            }
        }

        let (status, code, msg, full) = match attempt {
            Ok((s, c, m, f)) => (s, c, m, f),
            Err(e) => return Err(e),
        };

        let data = full.get("data");
        let access_token = data.and_then(|d| d.get("accessToken")).and_then(value_to_string);
        let refresh_token = data.and_then(|d| d.get("refreshToken")).and_then(value_to_string);
        let uid = data.and_then(|d| d.get("uid")).and_then(value_to_string);

        if !is_ok(status) || code != 0 || access_token.is_none() || refresh_token.is_none() || uid.is_none() {
            return Err(api_error("userCenterLogin", status, code, &msg));
        }
        Ok((access_token.unwrap(), refresh_token.unwrap(), uid.unwrap()))
    }

    async fn request_user_center_login(
        &self,
        token: &str,
        user_identity: &str,
        device_id: &str,
        profile: &str,
    ) -> Result<(u16, i64, String, serde_json::Value)> {
        let mut headers = HeaderMap::new();
        if profile == "official" {
            headers.insert("Accept", HeaderValue::from_static("application/json, text/plain, */*"));
            headers.insert("Authorization", HeaderValue::from_static(""));
            headers.insert("appVersion", HeaderValue::from_static(TAYGEDO_APP_VER));
            headers.insert("platform", HeaderValue::from_static("android"));
            headers.insert("uid", HeaderValue::from_static("0"));
            headers.insert("debug-uid", HeaderValue::from_static("3"));
            headers.insert("deviceId", HeaderValue::from_str(device_id).unwrap());
            headers.insert("ds", HeaderValue::from_str(&make_ds(now_seconds(), &make_nonce(), TAYGEDO_APP_VER)).unwrap());
        } else {
            headers.insert("authorization", HeaderValue::from_static(""));
            headers.insert("appversion", HeaderValue::from_static(TAYGEDO_COMPAT_APP_VERSION));
            headers.insert("platform", HeaderValue::from_static("android"));
            headers.insert("uid", HeaderValue::from_static(TAYGEDO_COMPAT_UID));
            headers.insert("deviceid", HeaderValue::from_str(device_id).unwrap());
        }
        headers.insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded"));
        headers.insert("User-Agent", HeaderValue::from_static(NATIVE_USER_AGENT));

        let body = form_encode_pairs([
            ("token", token),
            ("userIdentity", user_identity),
            ("appId", TAYGEDO_LOGIN_APP_ID),
        ]);

        let (status, text) = self
            .post_form(&format!("{TAYGEDO_BASE_URL}/usercenter/api/login"), headers, body)
            .await?;
        let (code, msg, full) = parse_body(&text, "userCenterLogin", status)?;
        Ok((status, code, msg, full))
    }

    // ---- 签到相关 ----

    pub async fn refresh_token(&self, refresh_token: &str, device_id: &str) -> Result<(String, String, Option<String>)> {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_str(refresh_token).unwrap());
        headers.insert("deviceid", HeaderValue::from_str(device_id).unwrap());
        headers.insert("appversion", HeaderValue::from_static("1.1.0"));
        headers.insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded"));
        headers.insert("User-Agent", HeaderValue::from_static(NATIVE_USER_AGENT));

        let (status, text) = self
            .post_form(&format!("{TAYGEDO_BASE_URL}/usercenter/api/refreshToken"), headers, String::new())
            .await?;

        if status == 402 {
            return Err(ApiError("REFRESH_REJECTED_402: refreshToken 已失效，请重新登录".into()));
        }

        let (code, msg, full) = parse_body(&text, "refreshToken", status)?;
        let data = full.get("data");
        let access_token = data.and_then(|d| d.get("accessToken")).and_then(value_to_string);
        let refresh_token = data.and_then(|d| d.get("refreshToken")).and_then(value_to_string);
        let uid = data.and_then(|d| d.get("uid")).and_then(value_to_string);

        if !is_ok(status) || code != 0 || access_token.is_none() || refresh_token.is_none() {
            return Err(api_error("refreshToken", status, code, &msg));
        }
        Ok((access_token.unwrap(), refresh_token.unwrap(), uid))
    }

    pub async fn get_bind_role(&self, access_token: &str, uid: &str, game_id: &str) -> Result<Option<(String, Option<String>)>> {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", HeaderValue::from_str(access_token).unwrap());
        let url = format!(
            "{TAYGEDO_BASE_URL}/apihub/api/getGameBindRole?uid={}&gameId={}",
            url_encode(uid),
            url_encode(game_id)
        );
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getBindRole", status)?;
        let data = full.get("data");
        if !is_ok(status) || code != 0 || data.is_none() {
            return Err(api_error("getBindRole", status, code, &msg));
        }
        let role_id = data.and_then(|d| d.get("roleId")).and_then(value_to_string);
        let role_name = data.and_then(|d| d.get("roleName")).and_then(value_to_string);
        Ok(role_id.map(|rid| (rid, role_name)))
    }

    pub async fn get_game_roles(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
        game_id: &str,
    ) -> Result<Vec<GameRoleRaw>> {
        let mut headers = HeaderMap::new();
        headers.insert("platform", HeaderValue::from_static("android"));
        headers.insert("authorization", HeaderValue::from_str(access_token).unwrap());
        headers.insert("uid", HeaderValue::from_str(uid).unwrap());
        headers.insert("deviceid", HeaderValue::from_str(device_id).unwrap());
        headers.insert("appversion", HeaderValue::from_static("1.1.0"));
        headers.insert("User-Agent", HeaderValue::from_static(NATIVE_USER_AGENT));

        let url = format!(
            "{TAYGEDO_BASE_URL}/usercenter/api/v2/getGameRoles?gameId={}",
            url_encode(game_id)
        );
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getGameRoles", status)?;

        let roles = full
            .get("data")
            .and_then(|d| d.get("roles"))
            .and_then(|r| r.as_array());
        if !is_ok(status) || code != 0 || roles.is_none() {
            return Err(api_error("getGameRoles", status, code, &msg));
        }

        let mut out = Vec::new();
        for role in roles.unwrap() {
            let role_id = role.get("roleId").and_then(value_to_string);
            if let Some(rid) = role_id {
                out.push(GameRoleRaw {
                    role_id: rid,
                    role_name: role.get("roleName").and_then(value_to_string),
                });
            }
        }
        Ok(out)
    }

    pub async fn get_game_record_cards(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
    ) -> Result<Vec<GameRecordCard>> {
        let (headers, url) = native_request(
            access_token,
            uid,
            device_id,
            "GET",
            "/apihub/api/getGameRecordCard",
            &[("uid", uid)],
        );
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getGameRecordCards", status)?;

        let cards = full.get("data").and_then(|d| d.as_array());
        if !is_ok(status) || code != 0 || cards.is_none() {
            return Err(api_error("getGameRecordCards", status, code, &msg));
        }

        let mut out = Vec::new();
        for card in cards.unwrap() {
            let game_id = card.get("gameId").and_then(value_to_string);
            if game_id.is_none() {
                continue;
            }
            let bind = card.get("bindRoleInfo");
            let role_id = bind.and_then(|b| b.get("roleId")).and_then(value_to_string);
            let role_name = bind.and_then(|b| b.get("roleName")).and_then(value_to_string);
            out.push(GameRecordCard {
                game_id: game_id.unwrap(),
                game_name: card.get("gameName").and_then(value_to_string),
                role_id,
                role_name,
            });
        }
        Ok(out)
    }

    pub async fn app_signin(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
    ) -> Result<(f64, f64)> {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_str(access_token).unwrap());
        headers.insert("uid", HeaderValue::from_str(uid).unwrap());
        headers.insert("deviceid", HeaderValue::from_str(device_id).unwrap());
        headers.insert("appversion", HeaderValue::from_static("1.1.0"));
        headers.insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded"));
        headers.insert("User-Agent", HeaderValue::from_static(NATIVE_USER_AGENT));

        let (status, text) = self
            .post_form(
                &format!("{TAYGEDO_BASE_URL}/apihub/api/signin"),
                headers,
                "communityId=1".into(),
            )
            .await?;
        let (code, msg, full) = parse_body(&text, "appSignin", status)?;

        let data = full.get("data");
        let exp = data.and_then(|d| d.get("exp")).and_then(value_to_f64);
        let gold = data.and_then(|d| d.get("goldCoin")).and_then(value_to_f64);

        if !is_ok(status) || code != 0 || exp.is_none() || gold.is_none() {
            return Err(api_error("appSignin", status, code, &msg));
        }
        Ok((exp.unwrap(), gold.unwrap()))
    }

    pub async fn get_signin_state(&self, access_token: &str, game_id: &str) -> Result<i64> {
        let (headers, url) = h5_request(access_token, "GET", "/apihub/awapi/signin/state", &[("gameId", game_id)]);
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getSigninState", status)?;
        let days = full
            .get("data")
            .and_then(|d| d.get("days"))
            .and_then(value_to_i64);
        if !is_ok(status) || code != 0 || days.is_none() {
            return Err(api_error("getSigninState", status, code, &msg));
        }
        Ok(days.unwrap())
    }

    pub async fn get_signin_rewards(&self, access_token: &str, game_id: &str) -> Result<Vec<(String, i64)>> {
        let (headers, url) = h5_request(access_token, "GET", "/apihub/awapi/sign/rewards", &[("gameId", game_id)]);
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getSigninRewards", status)?;
        let arr = full.get("data").and_then(|d| d.as_array());
        if !is_ok(status) || code != 0 || arr.is_none() {
            return Err(api_error("getSigninRewards", status, code, &msg));
        }
        let mut out = Vec::new();
        for item in arr.unwrap() {
            let name = item.get("name").and_then(value_to_string).unwrap_or_default();
            let num = item.get("num").and_then(value_to_i64).unwrap_or(0);
            out.push((name, num));
        }
        Ok(out)
    }

    pub async fn game_signin(&self, access_token: &str, role_id: &str, game_id: &str) -> Result<()> {
        let body = form_encode_pairs([("roleId", role_id), ("gameId", game_id)]);
        let (headers, url) = h5_request_with_body(access_token, "POST", "/apihub/awapi/sign");
        let (status, text) = self.post_form(&url, headers, body).await?;
        let (code, msg, _) = parse_body(&text, "gameSignin", status)?;
        if !is_ok(status) || code != 0 {
            return Err(api_error("gameSignin", status, code, &msg));
        }
        Ok(())
    }

    pub async fn get_user_tasks(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
    ) -> Result<Vec<CoinTask>> {
        let (headers, url) = native_request(
            access_token,
            uid,
            device_id,
            "GET",
            "/apihub/api/getUserTasks",
            &[("gid", "1")],
        );
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getUserTasks", status)?;
        let list = full.get("data").and_then(|d| d.get("task_list1")).and_then(|t| t.as_array());
        if !is_ok(status) || code != 0 || list.is_none() {
            return Err(api_error("getUserTasks", status, code, &msg));
        }
        let mut out = Vec::new();
        for t in list.unwrap() {
            let code_key = t.get("code").or_else(|| t.get("taskKey")).and_then(value_to_string);
            if let Some(code_key) = code_key {
                if code_key.is_empty() {
                    continue;
                }
                out.push(CoinTask {
                    code: code_key,
                    complete_times: t.get("completeTimes").and_then(value_to_i64).unwrap_or(0),
                    limit_times: t.get("limitTimes").and_then(value_to_i64).unwrap_or(0),
                });
            }
        }
        Ok(out)
    }

    pub async fn bbs_signin(&self, access_token: &str, uid: &str, device_id: &str) -> Result<()> {
        let (headers, url) = native_request_with_body(
            access_token,
            uid,
            device_id,
            "POST",
            "/apihub/api/signin",
            &[("communityId", "2")],
        );
        let body = form_encode_pairs([("communityId", "2")]);
        let (status, text) = self.post_form(&url, headers, body).await?;
        let (code, msg, _) = parse_body(&text, "bbsSignin", status)?;
        if !is_ok(status) || code != 0 {
            return Err(api_error("bbsSignin", status, code, &msg));
        }
        Ok(())
    }

    pub async fn get_recommend_post_list(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
    ) -> Result<Vec<RecommendPost>> {
        let (headers, url) = native_request(
            access_token,
            uid,
            device_id,
            "GET",
            "/bbs/api/getRecommendPostList",
            &[("communityId", "2"), ("count", "20"), ("page", "1")],
        );
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getRecommendPostList", status)?;

        let raw = full.get("data").and_then(|d| d.get("list")).and_then(|l| l.as_array())
            .or_else(|| full.get("data").and_then(|d| d.as_array()));
        if !is_ok(status) || code != 0 || raw.is_none() {
            return Err(api_error("getRecommendPostList", status, code, &msg));
        }
        let mut out = Vec::new();
        for item in raw.unwrap() {
            let post_id = item.get("postId").or_else(|| item.get("id")).and_then(value_to_string);
            if let Some(pid) = post_id {
                let liked = item
                    .get("selfOperation")
                    .and_then(|s| s.get("liked"))
                    .and_then(|l| l.as_bool());
                out.push(RecommendPost { post_id: pid, liked });
            }
        }
        Ok(out)
    }

    pub async fn get_post_full(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
        post_id: &str,
    ) -> Result<RecommendPost> {
        let (headers, url) = native_request(
            access_token,
            uid,
            device_id,
            "GET",
            "/bbs/api/getPostFull",
            &[("postId", post_id)],
        );
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getPostFull", status)?;
        if !is_ok(status) || code != 0 {
            return Err(api_error("getPostFull", status, code, &msg));
        }
        let liked = full
            .get("data")
            .and_then(|d| d.get("selfOperation"))
            .and_then(|s| s.get("liked"))
            .and_then(|l| l.as_bool());
        Ok(RecommendPost { post_id: post_id.to_string(), liked })
    }

    pub async fn like_post(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
        post_id: &str,
    ) -> Result<()> {
        let (headers, url) = native_request_with_body(
            access_token,
            uid,
            device_id,
            "POST",
            "/bbs/api/post/like",
            &[("postId", post_id)],
        );
        let body = form_encode_pairs([("postId", post_id)]);
        let (status, text) = self.post_form(&url, headers, body).await?;
        let (code, msg, _) = parse_body(&text, "likePost", status)?;
        if !is_ok(status) || code != 0 {
            return Err(api_error("likePost", status, code, &msg));
        }
        Ok(())
    }

    pub async fn share_post(
        &self,
        access_token: &str,
        uid: &str,
        device_id: &str,
        post_id: &str,
        platform: &str,
    ) -> Result<()> {
        let (headers, url) = native_request_with_body(
            access_token,
            uid,
            device_id,
            "POST",
            "/bbs/api/post/share",
            &[("platform", platform), ("postId", post_id)],
        );
        let body = form_encode_pairs([("platform", platform), ("postId", post_id)]);
        let (status, text) = self.post_form(&url, headers, body).await?;
        let (code, msg, _) = parse_body(&text, "sharePost", status)?;
        if !is_ok(status) || code != 0 {
            return Err(api_error("sharePost", status, code, &msg));
        }
        Ok(())
    }

    pub async fn get_user_coin_task_state(&self, access_token: &str) -> Result<CoinState> {
        let (headers, url) = h5_request(access_token, "GET", "/apihub/api/getUserCoinTaskState", &[]);
        let (status, text) = self.get_text(&url, headers).await?;
        let (code, msg, full) = parse_body(&text, "getUserCoinTaskState", status)?;
        let data = full.get("data");
        if !is_ok(status) || code != 0 || data.is_none() {
            return Err(api_error("getUserCoinTaskState", status, code, &msg));
        }
        Ok(CoinState {
            today_coin: data.and_then(|d| d.get("todayCoin")).and_then(value_to_f64),
            limit_coin: data.and_then(|d| d.get("limitCoin")).and_then(value_to_f64),
        })
    }

    pub async fn cloud_get_user_info(
        &self,
        laohu_token: &str,
        laohu_user_id: &str,
        device_id: &str,
    ) -> Result<CloudInfo> {
        let mut data = BTreeMap::new();
        data.insert("appId".into(), CLOUD_APP_ID.into());
        data.insert("deviceId".into(), device_id.into());
        data.insert("deviceType".into(), "Pixel 8".into());
        data.insert("deviceName".into(), "Pixel 8".into());
        data.insert("t".into(), now_seconds().to_string());
        data.insert("channelId".into(), CLOUD_CHANNEL_ID.into());
        data.insert("deviceModel".into(), "Pixel 8".into());
        data.insert("deviceSys".into(), "14".into());
        data.insert("version".into(), CLOUD_APP_VERSION.into());
        data.insert("sdkVersion".into(), CLOUD_SDK_VERSION.into());
        data.insert("network".into(), "wifi".into());
        data.insert("bid".into(), CLOUD_BID.into());
        data.insert("provider".into(), "0".into());
        data.insert("idfa".into(), String::new());
        data.insert("userId".into(), laohu_user_id.into());
        data.insert("token".into(), laohu_token.into());
        let body = signed_body(data, CLOUD_APP_KEY, true);

        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded"));
        headers.insert("User-Agent", HeaderValue::from_static("okhttp/3.12.1"));
        headers.insert("Host", HeaderValue::from_static("user.laohu.com"));

        let (status, text) = self
            .post_form(&format!("{LAOHU_BASE_URL}/cloud/game/getUserInfo"), headers, body)
            .await?;
        let (code, msg, full) = parse_body(&text, "cloudGetUserInfo", status)?;
        let result = full.get("result");
        if !is_ok(status) || code != 0 || result.is_none() {
            return Err(api_error("cloudGetUserInfo", status, code, &msg));
        }
        let gave = result
            .and_then(|r| r.get("perDayFirstLoginGiveDuration"))
            .and_then(value_to_f64)
            .unwrap_or(0.0);
        let remained = result
            .and_then(|r| r.get("remainedDuration"))
            .and_then(value_to_f64);
        Ok(CloudInfo { gave, remained })
    }

    // ---- 底层请求 ----

    async fn get_text(&self, url: &str, headers: HeaderMap) -> Result<(u16, String)> {
        let resp = self
            .client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ApiError(format!("请求失败：{e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| ApiError(format!("读取响应失败：{e}")))?;
        Ok((status, text))
    }

    async fn post_form(&self, url: &str, headers: HeaderMap, body: String) -> Result<(u16, String)> {
        let resp = self
            .client
            .post(url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| ApiError(format!("请求失败：{e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| ApiError(format!("读取响应失败：{e}")))?;
        Ok((status, text))
    }
}

impl Default for Api {
    fn default() -> Self {
        Self::new()
    }
}

// ---- 辅助函数 ----

fn laohu_headers() -> HeaderMap {
    let mut m = HeaderMap::new();
    m.insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded;charset=UTF-8"));
    m.insert("User-Agent", HeaderValue::from_static(LAOHU_USER_AGENT));
    m
}

fn native_common_headers(access_token: &str, uid: &str, device_id: &str) -> HeaderMap {
    let mut m = HeaderMap::new();
    m.insert("Accept", HeaderValue::from_static("application/json"));
    m.insert("Authorization", HeaderValue::from_str(access_token).unwrap());
    m.insert("appversion", HeaderValue::from_static(TAYGEDO_APP_VER));
    m.insert("platform", HeaderValue::from_static("android"));
    m.insert("uid", HeaderValue::from_str(uid).unwrap());
    m.insert("deviceid", HeaderValue::from_str(device_id).unwrap());
    m.insert("ds", HeaderValue::from_str(&make_ds(now_seconds(), &make_nonce(), TAYGEDO_APP_VER)).unwrap());
    m.insert("User-Agent", HeaderValue::from_static(NATIVE_USER_AGENT));
    m
}

fn native_request(
    access_token: &str,
    uid: &str,
    device_id: &str,
    _method: &str,
    path: &str,
    query: &[(&str, &str)],
) -> (HeaderMap, String) {
    let headers = native_common_headers(access_token, uid, device_id);
    let mut url = format!("{TAYGEDO_BASE_URL}{path}");
    if !query.is_empty() {
        let qs = form_encode_pairs(query.iter().copied());
        url = format!("{url}?{qs}");
    }
    (headers, url)
}

fn native_request_with_body(
    access_token: &str,
    uid: &str,
    device_id: &str,
    _method: &str,
    path: &str,
    _body: &[(&str, &str)],
) -> (HeaderMap, String) {
    let mut headers = native_common_headers(access_token, uid, device_id);
    headers.insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded"));
    (headers, format!("{TAYGEDO_BASE_URL}{path}"))
}

fn h5_headers(access_token: &str, with_body: bool) -> HeaderMap {
    let mut m = HeaderMap::new();
    m.insert("Accept", HeaderValue::from_static("application/json"));
    m.insert("Authorization", HeaderValue::from_str(access_token).unwrap());
    m.insert("Origin", HeaderValue::from_static(H5_ORIGIN));
    m.insert("Referer", HeaderValue::from_str(&format!("{H5_ORIGIN}/")).unwrap());
    m.insert("User-Agent", HeaderValue::from_static(H5_USER_AGENT));
    if with_body {
        m.insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded"));
    }
    m
}

fn h5_request(
    access_token: &str,
    _method: &str,
    path: &str,
    query: &[(&str, &str)],
) -> (HeaderMap, String) {
    let headers = h5_headers(access_token, false);
    let mut url = format!("{TAYGEDO_BASE_URL}{path}");
    if !query.is_empty() {
        let qs = form_encode_pairs(query.iter().copied());
        url = format!("{url}?{qs}");
    }
    (headers, url)
}

fn h5_request_with_body(access_token: &str, _method: &str, path: &str) -> (HeaderMap, String) {
    (h5_headers(access_token, true), format!("{TAYGEDO_BASE_URL}{path}"))
}

fn parse_body(text: &str, endpoint: &str, status: u16) -> Result<(i64, String, serde_json::Value)> {
    if text.trim().is_empty() {
        return Err(ApiError(format!("{endpoint} 返回了无效 JSON（HTTP {status}，响应为空）")));
    }
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| ApiError(format!("{endpoint} 返回了无效 JSON（HTTP {status}）")))?;
    let code = v.get("code").and_then(value_to_i64).unwrap_or(-1);
    let msg = v
        .get("msg")
        .or_else(|| v.get("message"))
        .and_then(value_to_string)
        .unwrap_or_default();
    Ok((code, msg, v))
}

fn api_error(endpoint: &str, status: u16, code: i64, msg: &str) -> ApiError {
    let m = msg.trim();
    if !m.is_empty() && m.to_lowercase() != "ok" {
        return ApiError(format!("{endpoint}：{m}（HTTP {status}，code={code}）"));
    }
    ApiError(format!("{endpoint} 请求失败（HTTP {status}，code={code}，msg={m}）"))
}

fn value_to_i64(v: &serde_json::Value) -> Option<i64> {
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(u) = v.as_u64() {
        return i64::try_from(u).ok();
    }
    if let Some(s) = v.as_str() {
        return s.parse().ok();
    }
    None
}

fn value_to_f64(v: &serde_json::Value) -> Option<f64> {
    if let Some(f) = v.as_f64() {
        return Some(f);
    }
    if let Some(i) = v.as_i64() {
        return Some(i as f64);
    }
    if let Some(s) = v.as_str() {
        return s.parse().ok();
    }
    None
}

fn value_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn is_ok(status: u16) -> bool {
    (200..300).contains(&status)
}

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
