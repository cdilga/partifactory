use crate::{Result, invalid};
use pf_db::{
    Db,
    rusqlite::{OptionalExtension, params},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedGrant {
    pub part_id: String,
    pub label: String,
    pub subject: String,
    pub role: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegrantPlan {
    pub format_version: u32,
    pub grants: Vec<ProposedGrant>,
    pub unmatched: Vec<Value>,
}
/// Labels are proposals only. Ambiguous people are never guessed, and portable labels propose
/// view access, requiring an explicit administrator review/apply before any access exists.
pub fn plan_regrant(db: &Db) -> Result<RegrantPlan> {
    db.read_blocking(|conn|{
        let encoded:Option<String>=conn.query_row("SELECT value FROM kv WHERE key='import.audience_labels'",[],|row|row.get(0)).optional()?;
        let labels:Vec<Value>=match encoded {Some(value)=>serde_json::from_str(&value)?,None=>{
            crate::portable::export(conn)?["audience_labels"].as_array().cloned().unwrap_or_default()
        }};
        let mut plan=RegrantPlan{format_version:1,grants:vec![],unmatched:vec![]};
        for label in labels {
            let name=label["label"].as_str().ok_or_else(||invalid("Malformed stored audience label."))?;
            let subject=match label["kind"].as_str(){
                Some("group")=>conn.query_row("SELECT 'group:'||name FROM groups WHERE name=?1",[name],|row|row.get::<_,String>(0)).optional()?,
                Some("everyone")=>Some("everyone-signed-in".into()),
                Some("person")=>{
                    let mut query=conn.prepare("SELECT 'user:'||id FROM users WHERE status='active' AND (username=?1 OR email=?1) ORDER BY id")?;
                    let matches=query.query_map([name],|row|row.get::<_,String>(0))?.collect::<pf_db::rusqlite::Result<Vec<_>>>()?;
                    if matches.len()==1{matches.into_iter().next()}else{None}
                },_=>None
            };
            match subject {Some(subject)=>plan.grants.push(ProposedGrant{part_id:label["part_id"].as_str().ok_or_else(||invalid("Malformed audience target."))?.into(),label:name.into(),subject,role:0}),None=>plan.unmatched.push(label)}
        }
        Ok::<_,crate::Error>(plan)
    })
}
pub fn apply_regrant(
    db: &Db,
    epochs: &pf_caps::EpochStore,
    plan: &RegrantPlan,
    actor_user_id: &str,
) -> Result<usize> {
    if plan.format_version != 1 {
        return Err(invalid("Unsupported regrant plan."));
    }
    let grants = plan.grants.clone();
    let actor = actor_user_id.to_owned();
    let now = pf_db::ids::now_ms();
    let (changed,bumps)=db.write_blocking(move|tx|{
        if !tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND status='active')",[&actor],|row|row.get::<_,bool>(0))?{return Err(invalid("Regrant requires an active administrator actor."));}
        let mut changed=0;let mut bumps=vec![];
        for grant in grants {
            if !(0..=3).contains(&grant.role){return Err(invalid("Invalid proposed role."));}
            let valid_subject=if let Some(user)=grant.subject.strip_prefix("user:"){tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND status='active')",[user],|row|row.get::<_,bool>(0))?}else if let Some(group)=grant.subject.strip_prefix("group:"){tx.query_row("SELECT EXISTS(SELECT 1 FROM groups WHERE name=?1)",[group],|row|row.get::<_,bool>(0))?}else{grant.subject=="everyone-signed-in"};
            if !valid_subject || !tx.query_row("SELECT EXISTS(SELECT 1 FROM parts WHERE id=?1)",[&grant.part_id],|row|row.get::<_,bool>(0))?{return Err(invalid("A proposed target or subject no longer exists."));}
            let inserted=tx.execute("INSERT INTO grants(target_kind,target_id,subject,role,created_by,created_at) VALUES('part',?1,?2,?3,?4,?5) ON CONFLICT(target_kind,target_id,subject) DO NOTHING",params![grant.part_id,grant.subject,grant.role,actor,now])?;
            if inserted>0 {
                tx.execute("INSERT INTO audit(at,user_id,actor,action,target_kind,target_id,after) VALUES(?1,?2,'system','import.regrant','part',?3,?4)",params![now,actor,grant.part_id,serde_json::to_string(&json!({"subject":grant.subject,"role":grant.role}))?])?;
                let key=pf_caps::part_key(&grant.part_id);let value=pf_caps::bump_epoch(tx,&key)?;bumps.push((key,value));changed+=1;
            }
        }
        Ok::<_,crate::Error>((changed,bumps))
    })?;
    for (key, value) in bumps {
        epochs.set(&key, value);
    }
    Ok(changed)
}
