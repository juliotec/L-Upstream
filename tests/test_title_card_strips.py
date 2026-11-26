"""The wide title-card frame uses independent strips without stretching its ornaments.

Run the real host helpers with synthetic constants and allocator/draw stand-ins. No game
file is read. Also covers mirrored quarters, immutable prepared choices, guards, full-picture
reaches and the full host planner. (Their lifted half, the hook sites that call them, is
tests/test_title_card_sites.py's.)

python -I -B tests/test_title_card_strips.py [--cc gcc]
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cc", "{cc}"]
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from srccut import between

FIXTURE = r'''
#include <stdint.h>
#include <math.h>
#include <assert.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; unsigned char v[512]; } Ctx;
static unsigned guest_reads;
static unsigned char memory[65536];
static uint64_t recomp_load8(Ctx*c,uint64_t p){(void)c;guest_reads++;assert(p<sizeof memory);return memory[p];}
static uint64_t recomp_load32(Ctx*c,uint64_t p){uint32_t v;(void)c;guest_reads++;assert(p+4<=sizeof memory);memcpy(&v,memory+p,4);return v;}
static uint64_t recomp_load64(Ctx*c,uint64_t p){uint64_t v;(void)c;guest_reads++;assert(p+8<=sizeof memory);memcpy(&v,memory+p,8);return v;}
static void recomp_store8(Ctx*c,uint64_t p,uint64_t v){(void)c;guest_reads++;assert(p<sizeof memory);memory[p]=(unsigned char)v;}
static void recomp_store32(Ctx*c,uint64_t p,uint32_t v){(void)c;guest_reads++;assert(p+4<=sizeof memory);memcpy(memory+p,&v,4);}
static int log_each;
static unsigned warnings_seen, guard_logs, trace_logs;
void hos_log(const char*f,...){char b[512];va_list a;va_start(a,f);vsnprintf(b,sizeof b,f,a);va_end(a);if(strstr(b,"unknown frame pane"))warnings_seen++;if(strstr(b,"guard failed"))guard_logs++;}
#define HOS_LOG_TRACE 0
void hos_log_at(int level,const char*f,...){assert(level==HOS_LOG_TRACE);(void)f;trace_logs++;}
@HOST@
static unsigned allocations,draws,draw_ids[64];
static unsigned char allocated[64][0x640],descs[64][24];
void f_8154f1c0(Ctx*c){unsigned n=allocations++;assert(n<64);memcpy(allocated[n],memory+c->x[0],0x640);memset(descs[n],0,24);memcpy(descs[n],&allocations,4);memcpy(memory+c->x[8],descs[n],24);c->x[0]=0x12345678;c->x[1]=0x87654321;}
void title_original_draw(Ctx*c){assert(draws<64);memcpy(&draw_ids[draws++],memory+c->x[0]+0x48,4);c->x[0]=777;}
static void u64(unsigned p,uint64_t v){memcpy(memory+p,&v,8);}
static void u32(unsigned p,uint32_t v){memcpy(memory+p,&v,4);}
static void near(float a,float b){assert(fabsf(a-b)<.002f);}
static Ctx ctx;
enum {PANE=0x1000,LAYOUT=0x2000,RES=0x3000,NAMES=0x4000,PICTURE=0x5000,PARAMS=0x6000,OUT=0x7000,PAIR=0x8000};
static void setup(const char*name,int right,int bottom){
 memset(memory,0,sizeof memory);memset(&ctx,0,sizeof ctx);memset(title_saved,0,sizeof title_saved);title_work=0;allocations=draws=0;
 u64(PANE+0x18,LAYOUT);u32(PANE+0xa8,1u<<22);u64(LAYOUT+0xf8,RES);u64(LAYOUT+0x100,PANE);u32(LAYOUT+0x13c,1);
 u64(RES+0x18,0x4100);strcpy((char*)memory+0x4100,"Location_00");u64(RES+0x20,NAMES);u64(NAMES,0x4200);strcpy((char*)memory+0x4200,name);u64(PICTURE+0x30,0x9000);u64(PAIR+8,PANE);
 u64(0x9030,0x9200);u32(0x9058,3u<<15);memory[0x9270]=memory[0x92c0]=6;
 unsigned char*p=memory+PARAMS;memset(p,0xa5,0x640);title_put(p+0x20,1);title_put(p+0xb0,1920);title_put(p+0xb4,1080);u32(PARAMS+0x630,3);
 title_put(p+0x108,512);title_put(p+0x190,512);
 for(int row=0;row<3;row++){title_put(p+0x40+16*row,row==0?1:0);title_put(p+0x4c+16*row,row==0?(right?960:-960):(bottom?-540:540));}
 for(int k=0;k<3;k++){unsigned o=(unsigned[]){0xc0,0x110,0x150}[k];float uv[8]={0,0,1,0,0,1,1,1};if(!k)for(int j=0;j<4;j++){if(right)uv[2*j]=1-uv[2*j];if(bottom)uv[2*j+1]=1-uv[2*j+1];}memcpy(p+o,uv,32);}
}
static void prep(void){ctx.x[0]=PARAMS;ctx.x[1]=0xaaaa;ctx.x[2]=16;ctx.x[8]=OUT;hos_wide_title_prepare(&ctx,PICTURE,PANE,PARAMS,ctx.x[1],ctx.x[2],OUT,f_8154f1c0);memcpy(memory+PICTURE+0x48,memory+OUT,24);}
static int draw(void){ctx.x[0]=PICTURE;ctx.x[1]=PAIR;int handled=hos_wide_title_draw(&ctx,title_original_draw);if(!handled)title_original_draw(&ctx);return handled;}
int main(void){
 const char*names[]={"P_DecoFrameLT_00","P_DecoFrameLTSh_00","P_DecoFrameRT_00","P_DecoFrameRTSh_00","P_DecoFrameLB_00","P_DecoFrameLBSh_00","P_DecoFrameRB_00","P_DecoFrameRBSh_00"};
 const float extensions[]={-300,-192,600,640,672,1920,3840};
 for(int k=0;k<8;k++)for(unsigned z=0;z<sizeof extensions/sizeof *extensions;z++){
  int right=(k/2)&1,bottom=k>=4;float ex=extensions[z];setup(names[k],right,bottom);title_plan_tick(ex);
  unsigned char original[0x640];memcpy(original,memory+PARAMS,sizeof original);prep();assert(allocations==4);assert(!memcmp(original,memory+PARAMS,sizeof original));assert(!memcmp(memory+OUT,descs[0],24));assert(ctx.x[0]==0x12345678&&ctx.x[1]==0x87654321);
  TitleStrip s[3];assert(title_strips(ex,right,s));float sum=0;
  for(int i=0;i<3;i++){
   const unsigned char*p=allocated[i+1];sum+=s[i].width;near(title_float(p+0x40),s[i].width/1920);near(title_float(p+0x4c),(right?960:-960)+s[i].centre);
   near(title_float(p+0xc0),right?1-s[i].source0:s[i].source0);near(title_float(p+0xc8),right?1-s[i].source1:s[i].source1);near(title_float(p+0xc4),bottom?1:0);near(title_float(p+0xd4),bottom?0:1);
   if(i<2){near(s[i].centre+s[i].width/2,s[i+1].centre-s[i+1].width/2);near(s[i].source1,s[i+1].source0);}
   near(title_float(p+0x110),s[i].source0);near(title_float(p+0x158),s[i].source1);
  }
  near(sum,1920+ex);near(s[right?2:0].width,384);near(s[right?0:2].width,168);
  assert(draw()&&draws==3);for(int i=0;i<3;i++)assert(draw_ids[i]==(unsigned)i+2);assert(!memcmp(memory+PICTURE+0x48,descs[0],24));
 }
 /* A new option after preparation cannot split a pane. One prepared frame may retain its choice. */
 setup(names[0],0,0);title_plan_tick(640);prep();title_plan_tick(0);assert(draw()&&draws==3);
 prep();assert(allocations==5);title_plan_tick(640);assert(!draw()&&draws==4&&draw_ids[3]==5);
 prep();assert(allocations==9);assert(draw()&&draws==7);
 /* Middle and 16:9 remain byte exact. Invalid widths never invert a strip. */
 {TitleStrip invalid[3];assert(!title_strips(-1368,0,invalid));assert(!title_strips(-1920,1,invalid));assert(!title_strips(NAN,0,invalid));}
 setup(names[0],0,0);title_plan_tick(0);unsigned char copy[0x640];memcpy(copy,memory+PARAMS,sizeof copy);prep();assert(allocations==1);assert(!memcmp(copy,allocated[0],sizeof copy));assert(!draw()&&draws==1&&draw_ids[0]==1);
 setup(names[0],0,0);title_plan_tick(640);title_put(memory+PARAMS+0x20,0);prep();assert(allocations==1);assert(!draw()&&draws==1);
 setup("P_DecoFrameLT_CHANGED",0,0);title_plan_tick(640);prep();assert(allocations==1&&warnings_seen==1);assert(!draw());
 setup("P_DecoLine_00",0,0);title_plan_tick(640);prep();assert(allocations==1&&warnings_seen==1);assert(!draw());
 /* Retained constants survive a pause, duplicate draws and option changes, until reset. */
 setup(names[0],0,0);title_plan_tick(640);prep();assert(draw());
 for(int i=0;i<100;i++)title_plan_tick(0);
 assert(draw()&&draws==6);assert(title_reuses[0]>0);
 hos_wide_title_reset(0xbbbb);assert(draw()&&draws==9);
 hos_wide_title_reset(0xaaaa);assert(!(title_work>>1));assert(!draw()&&draws==10);
 /* An identical descriptor after a reset must never revive recycled strips (ABA). */
 memcpy(memory+PICTURE+0x48,descs[0],24);assert(!draw()&&draws==11);
 /* An idle draw never follows any guest pointer. A narrow preparation only allocates. */
 ctx.x[0]=UINT64_MAX;ctx.x[1]=UINT64_MAX;guest_reads=0;
 assert(!hos_wide_title_draw(&ctx,title_original_draw));assert(guest_reads==0);
 setup(names[0],0,0);title_plan_tick(0);guest_reads=0;prep();assert(guest_reads==0);
 /* A wide screen between cards has no pending draw; a different picture also skips the name walk. */
 title_plan_tick(640);prep();ctx.x[0]=UINT64_MAX;ctx.x[1]=UINT64_MAX;guest_reads=0;
 assert(!hos_wide_title_draw(&ctx,title_original_draw));assert(guest_reads==0);
 /* Replacement before a draw is counted, once in the log per kind. */
 uint64_t unused=title_events[TITLE_UNUSED][0];prep();assert(title_events[TITLE_UNUSED][0]==unused+1);
 assert(draw());u32(PICTURE+0x48,999);assert(!draw());assert(title_events[TITLE_FALLBACK][0]==1);
 /* Repeated bad constants are retained by the game; only the first warning is printed. */
 setup(names[0],0,0);title_plan_tick(640);title_put(memory+PARAMS+0xb0,123);
 for(int i=0;i<100;i++){allocations=0;prep();}
 assert(guard_logs==1&&title_events[TITLE_GUARD][0]==100);
 /* Full storage is safe and bounded, too. */
 for(int i=0;i<128;i++){title_saved[i]=(TitlePrepared){.valid=1,.pane=(uint64_t)i+1,.picture=(uint64_t)i+1};}
 title_work=257;title_put(memory+PARAMS+0xb0,1920);allocations=0;prep();
 assert(title_events[TITLE_NO_SLOT][0]==1);
 log_each=1;memset(title_saved,0,sizeof title_saved);title_work=1;allocations=0;prep();draw();
 assert(trace_logs==2);
 /* Reuse of a picture address for another pane replaces its old record. */
 setup(names[0],0,0);title_plan_tick(640);prep();
 unsigned next=PANE+0xb0;u64(next+0x18,LAYOUT);u32(next+0xa8,1u<<22);
 u32(LAYOUT+0x13c,2);u64(NAMES+8,0x4200);u64(PAIR+8,next);
 ctx.x[0]=PARAMS;ctx.x[1]=0xaaaa;ctx.x[2]=16;ctx.x[8]=OUT;
 hos_wide_title_prepare(&ctx,PICTURE,next,PARAMS,ctx.x[1],ctx.x[2],OUT,f_8154f1c0);
 memcpy(memory+PICTURE+0x48,memory+OUT,24);
 assert((title_work>>1)==1);assert(draw()&&draws==3);
 /* A changed coordinate mode or invalid texture width retains the original. */
 for(int bad=0;bad<12;bad++){
  setup(names[0],0,0);title_plan_tick(640);
  if(bad==0)memory[0x9270]=0;
  if(bad==1)memory[0x92c1]=1;
  if(bad==2)u64(0x9030,0);
  if(bad==3)title_put(memory+PARAMS+0x108,0);
  if(bad==4)title_put(memory+PARAMS+0x190,NAN);
  if(bad==5)title_put(memory+PARAMS+0x190,INFINITY);
  if(bad==6)title_put(memory+PARAMS+0x190,-1);
  if(bad==7)u32(0x9058,2u<<15);
  if(bad==8)u64(PICTURE+0x30,0);
  if(bad==9)memory[0x9220]=1;
  if(bad==10)title_put(memory+PARAMS+0xb0,NAN);
  if(bad==11)title_put(memory+PARAMS+0xb4,INFINITY);
  prep();assert(allocations==1);assert(!draw()&&draws==1&&draw_ids[0]==1);
 }
 puts("PASS: actual strip helpers, all eight names, mirrored geometry/UVs, seven reaches including narrow pictures, independent data, restoration, prepared choice across option changes, no-op bytes, retained and reset state, bounded logs, idle fast paths, hidden pane, warning and banner guards");return 0;
}

'''

def run(cc, source):
    with tempfile.TemporaryDirectory(prefix="title-card-test-") as tmp:
        p = Path(tmp) / "test.c"
        exe = Path(tmp) / "test.exe"
        p.write_text(source, encoding="utf-8")
        subprocess.run([cc, "-std=c11", "-O1", "-Wall", "-Wextra", "-Wno-unused-function",
                        "-Wno-unused-variable", "-Werror", "-ffp-contract=off", str(p), "-lm", "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--cc", default=os.environ.get("CC", "gcc"))
    args = p.parse_args()
    for key in ("HOS_WIDE_UI", "HOS_HUD_MAX_RATIO", "HOS_HUD_FULL_RATIO"):
        os.environ.pop(key, None)
    host = (ROOT / "game/hos_wide_ui.c").read_text(encoding="utf-8")
    # (the strip helpers, the host's half. Their lifted half, the sites ui.titlePrepare, ui.titleDraw and ui.titleReset
    # at f_815505b0's call of f_8154f1c0, f_81550720's entry and f_8156abc0's, is tests/test_title_card_sites.py's)
    block = between(host, "/* The area title card's baked frame", "void hos_wide_ui_tick(void)")
    run(args.cc, FIXTURE.replace("@HOST@", block))

    import test_hud_layout as hud
    body = "".join(line for line in host.splitlines(True) if not line.startswith("#include"))
    body = body.replace("extern volatile float hos_wide_ratio;\n", "", 1)
    extra = r'''
    { const float ratios[]={0,16.0f/9,21.0f/9,64.0f/27,32.0f/9,48.0f/9,16.0f/10,4.0f/3};
      const float expected[]={0,0,600,640,1920,3840,-192,-300};
      /* Screen's edges and Custom use the whole picture for the large frame;
         the HUD checks above still require the ordinary HUD's limited reach. */
      for(int layout=0;layout<3;layout++) {
        fixture_hud_layout=layout;
        for(int i=0;i<8;i++) {
          at(ratios[i]);uint32_t u=(uint32_t)title_plan_read();float e;memcpy(&e,&u,4);
          CHECK(near(e,layout==1?0:expected[i]));
        }
      }
      fixture_hud_layout=0;at(64.0f/27);uint32_t u=(uint32_t)title_plan_read();float e;memcpy(&e,&u,4);CHECK(near(e,640));
    }
    { const uint64_t location=BASE+0x11000;
      layout_named(location,"Location_00");
      uint64_t root=pane_at(location,32,"RootPane",0,0,0);
      uint64_t top=pane_at(location,33,"P_DecoFrameLT_00",1920,1080,1);
      uint64_t bottom=pane_at(location,34,"P_DecoFrameLB_00",1920,1080,1);
      const float ratios[]={4.0f/3,16.0f/9,64.0f/27};
      const float ys[]={540,540,540};
      for(int k=0;k<3;k++) {
        hos_wide_ratio=ratios[k];
        for(int j=0;j<6;j++) {
          hos_wide_ui_tick();place(root,0,0,0);
          place(top,root,-960,540);if(j==5){CHECK(near(px,-960));CHECK(near(py,ys[k]));}
          place(bottom,root,-960,-540);if(j==5){CHECK(near(px,-960));CHECK(near(py,-ys[k]));}
        }
      }
    }
    '''
    env = '#include "' + hud.ENV_ON_H.replace("\\", "/") + '"\n'
    run(args.cc, env + hud.FIXTURE_HEAD + body + hud.FIXTURE_MAIN.replace("    return 0;", extra + "    return 0;"))
    print("PASS: actual host planner, all shapes and live HUD option")


if __name__ == "__main__":
    main()
