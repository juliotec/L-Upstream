"""DLSS 5's composite (vk_dlss.cpp nr_composite, shaders/dlss_nr_composite.comp) on the CPU, no GPU or game.

The Vulkan fixture records the real descriptor target and push constants. The sampling fixture executes the source-cut
GLSL sampling and debug branch with bounds-checked textures, including output larger than render/NR inputs. The blend
fixture executes the source-cut composition (the detail and colour strengths and the highlight guard, as the Bloodborne
port composes them) on chosen pixels.
"""
RUN_ALL_ARGS = ["--cxx", "{cxx}"]
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parent))
from srccut import block, between
ROOT=Path(__file__).resolve().parents[1]

BLEND = r'''
#include <algorithm>
#include <cassert>
#include <cmath>
#include <cstdio>
namespace glsl {   // (GLSL's types and functions, as much as the cut needs)
struct vec3 {
    double x,y,z;
    vec3():x(0),y(0),z(0){} explicit vec3(double v):x(v),y(v),z(v){} vec3(double a,double b,double c):x(a),y(b),z(c){}
    double& operator[](int i) { return i==0 ? x : i==1 ? y : z; }
    vec3& operator*=(double s) { x*=s;y*=s;z*=s;return *this; }
};
vec3 operator+(vec3 a,vec3 b) { return {a.x+b.x,a.y+b.y,a.z+b.z}; }
vec3 operator-(vec3 a,vec3 b) { return {a.x-b.x,a.y-b.y,a.z-b.z}; }
vec3 operator*(vec3 a,vec3 b) { return {a.x*b.x,a.y*b.y,a.z*b.z}; }
vec3 operator*(vec3 a,double s) { return {a.x*s,a.y*s,a.z*s}; }
vec3 operator*(double s,vec3 a) { return a*s; }
double dot(vec3 a,vec3 b) { return a.x*b.x+a.y*b.y+a.z*b.z; }
vec3 mix(vec3 a,vec3 b,double t) { return a+(b-a)*t; }
double clamp(double v,double lo,double hi) { return std::min(std::max(v,lo),hi); }
double max(double a,double b) { return std::max(a,b); }
double min(double a,double b) { return std::min(a,b); }
vec3 max(vec3 a,vec3 b) { return {std::max(a.x,b.x),std::max(a.y,b.y),std::max(a.z,b.z)}; }
double pow(double a,double b) { return std::pow(a,b); }
vec3 pow(vec3 a,vec3 b) { return {std::pow(a.x,b.x),std::pow(a.y,b.y),std::pow(a.z,b.z)}; }
vec3 abs(vec3 a) { return {std::fabs(a.x),std::fabs(a.y),std::fabs(a.z)}; }
vec3 sign(vec3 a) { auto s=[](double v){ return double((v>0)-(v<0)); }; return {s(a.x),s(a.y),s(a.z)}; }
struct mat3 {   // (GLSL's: column-major)
    double m[9];
    mat3(double a,double b,double c,double d,double e,double f,double g,double h,double i):m{a,b,c,d,e,f,g,h,i}{}
};
vec3 operator*(const mat3& M,vec3 v) {
    return {M.m[0]*v.x+M.m[3]*v.y+M.m[6]*v.z,M.m[1]*v.x+M.m[4]*v.y+M.m[7]*v.z,M.m[2]*v.x+M.m[5]*v.y+M.m[8]*v.z};
}
const vec3 Y = vec3(0.2126, 0.7152, 0.0722);
struct { double detail=1,color=1,guard=8; } pc;
OKLAB
vec3 compose(vec3 al,vec3 bl) {
    vec3 c;
    {
COMPOSE
    return c;
}
bool near(vec3 a,vec3 b,double e) { return std::fabs(a.x-b.x)<e && std::fabs(a.y-b.y)<e && std::fabs(a.z-b.z)<e; }
double light(vec3 c) { return dot(c,Y); }
} // namespace glsl
using namespace glsl;
int main() {
    const vec3 skin(0.40,0.22,0.15), grey(0.2,0.2,0.2), dark(0.001,0.001,0.001);
    // (OkLab there and back)
    assert(near(from_oklab(to_oklab(skin)),skin,1e-7));
    // detail 0: the upscaler's picture, whatever the model says, and whatever the colour strength (above 1 too:
    // none of the model's picture, so none of its extra chroma)
    pc.detail=0;pc.guard=2;
    for (double colour : {0.0,1.0,1.26,2.0}) {
        pc.color=colour;
        assert(near(compose(skin,vec3(0.9,0.1,0.1)),skin,1e-9));
    }
    // detail 1, colour 1, a guard it never reaches: the model's picture
    pc.detail=1;pc.color=1;pc.guard=8;
    const vec3 model(0.45,0.25,0.16);
    assert(near(compose(skin,model),model,1e-9));
    // colour 0: the picture's own hue at the model's light
    pc.color=0;
    vec3 c=compose(skin,model);
    assert(std::fabs(c.x/c.y-skin.x/skin.y)<1e-9 && std::fabs(c.z/c.y-skin.z/skin.y)<1e-9);
    assert(std::fabs(light(c)-light(model))<1e-3);
    // the guard: a model four times brighter is held to twice the light, and four times darker to half
    pc.color=1;pc.guard=2;
    assert(std::fabs(light(compose(grey,grey*4.0))/light(grey)-2.0)<2e-2);   // (within the ratio's small floor)
    assert(std::fabs(light(compose(grey,grey*0.25))/light(grey)-0.5)<2e-2);
    // detail above 1 raises the change in light to a power: 1.5 brighter at detail 2 is 2.25 brighter (guard 8)
    pc.detail=2;pc.guard=8;
    assert(std::fabs(light(compose(grey,grey*1.5))/light(grey)-2.25)<1e-2);
    // ... and leaves an unchanged pixel alone
    assert(near(compose(skin,skin),skin,1e-9));
    // a dark pixel's ratio eases to one (the floor): a small edit there passes whole, not clamped to the guard and back
    // each frame, where the same ratio on a lit pixel is held to it
    pc.detail=1;pc.guard=2;
    assert(std::fabs(light(compose(dark,dark*3.0))-light(dark*3.0))<1e-9);
    assert(light(compose(grey,grey*3.0))<light(grey)*2.05);
    // an empty answer: the picture as it was
    assert(near(compose(skin,vec3(0)),skin,1e-9));
    // colour above 1: more chroma at the same light, within 0..1
    pc.color=1.5;pc.guard=8;
    vec3 vivid=compose(skin,model);
    assert(std::fabs(light(vivid)-light(model))<2e-2);
    assert(vivid.x>=0 && vivid.y>=0 && vivid.z>=0 && vivid.x<=1 && vivid.y<=1 && vivid.z<=1);
    assert((vivid.x-vivid.z)>(model.x-model.z));
    // ... in step with the detail strength up to 1: at detail 0.5, half the extra chroma of the halfway picture (1.25
    // for 1.5); from detail 1, all of it (detail 2, an unchanged pixel: the colour strength's chroma alone, as the
    // Bloodborne port)
    pc.detail=0.5;
    const vec3 halfway=mix(skin,model,0.5);
    vec3 half_boost=compose(skin,model), lab=to_oklab(halfway);
    const vec3 want=from_oklab(vec3(lab.x,lab.y*1.25,lab.z*1.25));
    assert(near(half_boost,want,1e-6));
    pc.detail=2;
    lab=to_oklab(skin);
    assert(near(compose(skin,skin),from_oklab(vec3(lab.x,lab.y*1.5,lab.z*1.5)),1e-6));
    std::puts("blend: PASS");
}
'''


def main():
    p=argparse.ArgumentParser()
    p.add_argument('--cxx',default='g++')
    args=p.parse_args()
    source=(ROOT/'gpu/vk/vk_dlss.cpp').read_text(encoding='utf-8')
    shader=(ROOT/'gpu/vk/shaders/dlss_nr_composite.comp').read_text(encoding='utf-8')
    target=r'''
#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstring>
using VkCommandBuffer=int; using VkDescriptorType=int;
enum { VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, VK_DESCRIPTOR_TYPE_STORAGE_IMAGE,
       VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO, VK_FILTER_NEAREST, VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE,
       VK_SUCCESS, VK_FORMAT_R16G16B16A16_SFLOAT, VK_IMAGE_LAYOUT_GENERAL, VK_NULL_HANDLE,
       VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET, VK_PIPELINE_BIND_POINT_COMPUTE, VK_SHADER_STAGE_COMPUTE_BIT };
struct TemporalImage { int view=0; uint32_t width=20,height=10; };
struct Image : TemporalImage {};
struct State {
    int nr_comp=1,nr_set=1,nr_layout=1,point=1;
    bool nr_hdr_active=false,nr_scaled=false;
    uint32_t nr_original_w=20,nr_original_h=10;
    struct { int debug=1,detail=154,color=113,highlight=20; } nr_controls;
    TemporalImage color{3,10,5},depth{4,10,5},motion{5,10,5},nr_output{6,10,5},nr_hdr_original{2,30,15};
    Image nr_composed;
} state;
State& st() { return state; }
struct { int device=0; } dev;
struct VkSamplerCreateInfo { int sType=0,magFilter=0,minFilter=0,addressModeU=0,addressModeV=0,addressModeW=0; };
int vkCreateSampler(int,const VkSamplerCreateInfo*,void*,int*) { return VK_SUCCESS; }
bool make_image(int,Image& im,int,uint32_t w,uint32_t h) { im.view=99;im.width=w;im.height=h;return true; }
#define BUILTIN_SPV_ARGS(words) words,sizeof words
const int DLSS_NR_COMPOSITE_COMP_SPV[]={0};
bool make_pipeline(const int*,size_t,const int*,int,int,int&,int&,int&) { return true; }
struct VkDescriptorImageInfo { int sampler,view,layout; };
struct VkWriteDescriptorSet { int sType=0,dstBinding=0,descriptorCount=0,descriptorType=0; const VkDescriptorImageInfo* pImageInfo=nullptr; };
int written=0,original=0; int push[11]{};
float pushed(int i) { float f; std::memcpy(&f,&push[i],4); return f; }
void full_barrier(int) {}
void vkCmdBindPipeline(int,int,int) {}
void vkCmdPushDescriptorSetKHR(int,int,int,int,int n,VkWriteDescriptorSet* writes) {
    assert(n==6);written=writes[4].pImageInfo->view;original=writes[5].pImageInfo->view;
}
void vkCmdPushConstants(int,int,int,int,size_t n,const void* data) { assert(n==sizeof push);std::memcpy(push,data,n); }
void vkCmdDispatch(int,uint32_t,uint32_t,int) {}
FUNCTION
int main() {
    TemporalImage output{2,30,15};
    for(bool hdr : {false,true}) {
        state={}; state.nr_hdr_active=hdr;state.nr_scaled=!hdr;state.nr_hdr_original=output;
        state.nr_original_w=30;state.nr_original_h=15;
        assert(nr_composite(0,10,5,true));
        assert(written==state.nr_composed.view && written!=0 && written!=output.view);
        assert(state.nr_composed.width==30 && state.nr_composed.height==15 && original==2);
        assert(push[0]==30 && push[1]==15 && push[2]==10 && push[3]==5);
    }
    // (the strengths and the guard as the shader reads them: hundredths and tenths to floats, clamped)
    assert(pushed(6)==1.54f && pushed(7)==1.13f && pushed(8)==2.0f && push[9]==1);
    state.nr_controls.detail=-5;state.nr_controls.color=999;state.nr_controls.highlight=1;
    assert(nr_composite(0,10,5,true));
    assert(pushed(6)==0.0f && pushed(7)==2.0f && pushed(8)==1.0f);
    state={};state.nr_scaled=true;state.nr_hdr_original={7,40,20};
    assert(nr_composite(0,10,5,true));
    assert(written==state.nr_composed.view); // (debug views too: never the upscaler's own picture)
    assert(push[4]==20 && push[5]==10); // Active render subrect, not the larger allocation.
    state.nr_controls.debug=0;
    assert(nr_composite(0,10,5,true));
    assert(written==state.nr_composed.view && state.nr_composed.width==40);
    // (the picture the model saw, full size and SDR: the composite works at its size)
    state={};state.nr_controls.debug=0;
    assert(nr_composite(0,10,5,true));
    assert(state.nr_composed.width==10 && state.nr_composed.height==5 && push[10]==0 && original==3);
}
'''.replace('FUNCTION',block(source,'bool nr_composite('))
    sampling=r'''
#include <algorithm>
#include <cassert>
#include <cmath>
struct vec2;
struct ivec2 {
    int x,y;
    explicit ivec2(int v):x(v),y(v){} ivec2(int a,int b):x(a),y(b){}
    explicit ivec2(vec2 v);
};
struct vec2 {
    float x,y;
    vec2(float a,float b):x(a),y(b){} explicit vec2(ivec2 v):x(float(v.x)),y(float(v.y)){}
};
ivec2::ivec2(vec2 v):x(int(v.x)),y(int(v.y)){}
vec2 operator*(vec2 a,vec2 b) { return {a.x*b.x,a.y*b.y}; }
vec2 operator-(vec2 a,float b) { return {a.x-b,a.y-b}; }
ivec2 operator+(ivec2 a,ivec2 b) { return {a.x+b.x,a.y+b.y}; }
ivec2 operator-(ivec2 a,int b) { return {a.x-b,a.y-b}; }
vec2 floor(vec2 v) { return {std::floor(v.x),std::floor(v.y)}; }
vec2 fract(vec2 v) { return {v.x-std::floor(v.x),v.y-std::floor(v.y)}; }
ivec2 clamp(ivec2 v,ivec2 lo,ivec2 hi) { return {std::clamp(v.x,lo.x,hi.x),std::clamp(v.y,lo.y,hi.y)}; }
struct vec4 { float x,y,z,w; };
vec4 mix(vec4 a,vec4 b,float t) { return {a.x+(b.x-a.x)*t,a.y+(b.y-a.y)*t,a.z+(b.z-a.z)*t,a.w+(b.w-a.w)*t}; }
struct sampler2D { int width,height; } hdr_original{40,20};
ivec2 textureSize(sampler2D t,int) { return {t.width,t.height}; }
vec4 texelFetch(sampler2D t,ivec2 p,int) {
    assert(p.x>=0 && p.y>=0 && p.x<t.width && p.y<t.height);
    return {float(p.x),float(p.y),1,1};
}
struct { int debug_view=1,hdr=2; ivec2 input_size{10,5},original_size{20,10}; } pc;
ivec2 dst{29,14};vec2 uv{29.5f/30,14.5f/15};
vec4 a{-1,-2,-3,1},stored{};
int result=0;
void imageStore(int,ivec2,vec4 value) { stored=value; }
SAMPLE
void debug_pixel() {
DEBUG
    assert(false);
}
int main() {
    for(int hdr : {1,2}) {
        pc.hdr=hdr;
        for(int y=0;y<15;++y) for(int x=0;x<30;++x) {
            dst={x,y};uv={(x+.5f)/30,(y+.5f)/15};
            debug_pixel();
            assert(std::abs(stored.x-std::clamp(uv.x*20-.5f,0.f,19.f))<.0001f);
            assert(std::abs(stored.y-std::clamp(uv.y*10-.5f,0.f,9.f))<.0001f);
        }
    }
    pc.hdr=0;debug_pixel();assert(stored.x==a.x && stored.y==a.y);
}
'''.replace('SAMPLE',block(shader,'vec4 sample_low(')).replace('DEBUG',block(shader,'    if (pc.debug_view == 1)'))
    blend=BLEND.replace('OKLAB',block(shader,'vec3 cbrt_signed(')+'\n'+block(shader,'vec3 to_oklab(')+'\n'+
                        block(shader,'vec3 from_oklab('))
    blend=blend.replace('COMPOSE',between(shader,'        float ya = dot(al, Y);','    if (pc.hdr != 0 && pc.debug_view == 0) {'))
    failed=[]
    with tempfile.TemporaryDirectory(prefix='nr-composite-') as tmp:
        for name,fixture in [('target',target),('sampling',sampling),('blend',blend)]:
            cpp=Path(tmp)/(name+'.cpp')
            exe=Path(tmp)/(name+'.exe')
            cpp.write_text(fixture,encoding='utf-8')
            subprocess.run([args.cxx,'-std=c++20','-Wall','-Wextra','-Werror','-Wno-unused-function',str(cpp),'-o',str(exe)],check=True)
            completed=subprocess.run([str(exe)],capture_output=True,text=True,timeout=10)
            if completed.returncode:
                failed.append(name)
                print(completed.stdout+completed.stderr)
    assert not failed, 'NR composite regressions: '+', '.join(failed)
    print('PASS: actual composite target and push constants, bounds-checked Original sampling at different extents, the blend')
if __name__=='__main__':
    main()
