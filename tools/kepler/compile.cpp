/*
 * Copyright 2010 Christoph Bumiller
 * Copyright 2017 Red Hat Inc.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */

// Host catalogue frontend; control flow and SPH fields adapted from Mesa
// nv50_ir_from_nir.cpp and nvc0_program.c at the revision in sources.json.
#include "nv50_ir.h"
#include "nv50_ir_target.h"
#include "nv50_ir_build_util.h"
#include <array>
#include <fstream>
#include <sstream>
#include <regex>
#include <stdexcept>
#include <string>
#include <algorithm>

using namespace nv50_ir;
using Vec = std::array<Value *, 4>;
static void require(bool ok, const std::string &why) {
   if (!ok) throw std::runtime_error(why);
}
static std::string trim(std::string s) {
   auto a = s.find_first_not_of(" \t\r\n\0", 0, 5);
   if (a == std::string::npos) return "";
   return s.substr(a, s.find_last_not_of(" \t\r\n\0", std::string::npos, 5) - a + 1);
}
static std::vector<std::string> split(const std::string &s, char sep) {
   std::vector<std::string> r; std::stringstream in(s); std::string x;
   while (std::getline(in, x, sep)) r.push_back(trim(x));
   return r;
}
struct Operand {
   std::string file, swizzle;
   unsigned index;
   explicit Operand(const std::string &s) {
      static const std::regex re("(TEMP|IN|OUT|CONST|IMM|SAMP)\\[([0-9]+)\\](?:\\.([xyzw]+))?");
      std::smatch m;
      require(std::regex_match(s, m, re), "unsupported operand: " + s);
      file=m[1]; index=std::stoul(m[2]); swizzle=m[3];
      require(index < 4096, "register index too large");
   }
};

class Frontend : public BuildUtil {
   std::map<unsigned, Vec> temps, outputs;
   std::map<unsigned, std::array<uint32_t,4>> imms;
   std::map<unsigned, unsigned> inputAddress, outputAddress;
   std::map<unsigned, unsigned> inputMask, outputMask;
   std::vector<std::string> lines;
   unsigned cursor=0;
   BasicBlock *exitBlock;
   unsigned loopDepth=0;
   std::vector<BasicBlock *> loopTails;
   bool vertex, discard=false;
   unsigned samplers=0, cbBytes=0;

   Vec &reg(std::map<unsigned,Vec> &regs, unsigned i) {
      auto [it, added] = regs.emplace(i, Vec{});
      if (added) for (auto &v : it->second) v=getScratch();
      return it->second;
   }
   Value *read(const Operand &o, unsigned c) {
      if (!o.swizzle.empty()) {
         require(o.swizzle.size()==1 || o.swizzle.size()==4, "source swizzle must have 1 or 4 components");
         c=std::string("xyzw").find(o.swizzle[o.swizzle.size()==1 ? 0 : c]);
      }
      if (o.file=="TEMP") return reg(temps,o.index)[c];
      if (o.file=="OUT") return reg(outputs,o.index)[c];
      if (o.file=="IMM") { require(imms.count(o.index), "undeclared immediate"); return loadImm(getSSA(),imms.at(o.index)[c]); }
      if (o.file=="CONST") {
         cbBytes=std::max(cbBytes,o.index*16+(c+1)*4);
         return mkLoadv(TYPE_U32,mkSymbol(FILE_MEMORY_CONST,0,TYPE_U32,o.index*16+c*4),NULL);
      }
      require(o.file=="IN" && inputAddress.count(o.index), "undeclared input");
      inputMask[o.index] |= 1u<<c;
      Value *v=getSSA();
      unsigned address=inputAddress.at(o.index)+c*4;
      if (vertex) mkFetch(v,TYPE_F32,FILE_SHADER_INPUT,address,NULL,NULL);
      else mkInterp(NV50_IR_INTERP_LINEAR,v,address,NULL);
      return v;
   }
   void write(const Operand &o, unsigned c, Value *v) {
      require(o.file=="TEMP" || o.file=="OUT", "invalid destination");
      if (o.file=="OUT") {
         require(outputAddress.count(o.index), "undeclared output");
         outputMask[o.index] |= 1u<<c;
      }
      mkMov(reg(o.file=="OUT" ? outputs : temps,o.index)[c],v);
   }
   BasicBlock *block() { return new BasicBlock(getProgram()->main); }
   void branch(BasicBlock *to, Graph::Edge::Type kind=Graph::Edge::FORWARD) {
      if (!getBB()->isTerminated()) {
         mkFlow(OP_BRA,to,CC_ALWAYS,NULL);
         getBB()->cfg.attach(&to->cfg,kind);
      }
   }
   std::string opName(const std::string &line) { return line.substr(0,line.find(' ')); }
   void declaration(const std::string &line) {
      if (line.rfind("DCL IN[",0)==0 || line.rfind("DCL OUT[",0)==0) {
         auto args=split(line.substr(4),','); Operand o(args[0]);
         require(args.size()>=2 || (vertex && o.file=="IN"), "missing input semantic");
         unsigned address;
         if (vertex && o.file=="IN") address=0x80+o.index*16;
         else if (args[1]=="POSITION") address=0x70;
         else if (args[1]=="GENERIC[0]") address=0x80;
         else if (!vertex && o.file=="OUT" && args[1]=="COLOR") address=0;
         else throw std::runtime_error("unsupported semantic: "+line);
         (o.file=="IN" ? inputAddress : outputAddress)[o.index]=address;
      } else if (line.rfind("DCL ",0)==0) {
         // The operand parser accepts only direct indices; these declarations
         // reserve no runtime state. Unsupported instructions still fail below.
         require(line.rfind("DCL TEMP[",0)==0 || line.rfind("DCL CONST[",0)==0 ||
                 line.rfind("DCL SAMP[",0)==0 || line.rfind("DCL SVIEW[",0)==0,
                 "unsupported declaration: "+line);
      } else if (line.rfind("IMM[",0)==0) {
         auto a=line.find(' '); Operand o(line.substr(0,a));
         auto begin=line.find('{'), end=line.find('}');
         require(begin!=std::string::npos && end!=std::string::npos,"invalid immediate");
         auto v=split(line.substr(begin+1,end-begin-1),','); require(v.size()==4,"invalid immediate length");
         bool f=line.find("FLT32")!=std::string::npos;
         require(f || line.find("UINT32")!=std::string::npos || line.find("INT32")!=std::string::npos,"unknown immediate type");
         std::array<uint32_t,4> words;
         for (unsigned c=0;c<4;++c) {
            if (f) { float x=std::stof(v[c]); memcpy(&words[c],&x,4); }
            else words[c]=uint32_t(std::stoll(v[c]));
         }
         imms[o.index]=words;
      } else throw std::runtime_error("invalid declaration: "+line);
   }
   void conditional(const std::string &line) {
      Operand src(trim(line.substr(4)));
      Value *condition=read(src,0);
      BasicBlock *head=getBB(), *yes=block(), *no=block(), *tail=block();
      head->cfg.attach(&yes->cfg,Graph::Edge::TREE);
      head->cfg.attach(&no->cfg,Graph::Edge::TREE);
      mkFlow(OP_BRA,no,CC_EQ,condition)->setType(TYPE_U32);
      setPosition(yes,true);
      sequence(); branch(tail);
      require(cursor<lines.size(),"unterminated UIF");
      if (opName(lines[cursor])=="ELSE") {
         ++cursor; setPosition(no,true); sequence(); branch(tail);
      } else { setPosition(no,true); branch(tail); }
      require(cursor<lines.size() && lines[cursor]=="ENDIF","missing ENDIF"); ++cursor;
      // Match Mesa's reconvergence insertion: JOINAT precedes the branch.
      setPosition(head->getExit(),false);
      head->joinAt=mkFlow(OP_JOINAT,tail,CC_ALWAYS,NULL);
      setPosition(tail,true); mkFlow(OP_JOIN,NULL,CC_ALWAYS,NULL)->fixed=1;
   }
   void loop() {
      BasicBlock *body=block(), *tail=block();
      getBB()->cfg.attach(&body->cfg,Graph::Edge::TREE);
      mkFlow(OP_PREBREAK,tail,CC_ALWAYS,NULL);
      setPosition(body,true); mkFlow(OP_PRECONT,body,CC_ALWAYS,NULL);
      loopTails.push_back(tail); ++loopDepth;
      getFunction()->loopNestingBound=std::max(getFunction()->loopNestingBound,loopDepth);
      sequence();
      require(cursor<lines.size() && lines[cursor]=="ENDLOOP","missing ENDLOOP"); ++cursor;
      if (!getBB()->isTerminated()) {
         mkFlow(OP_CONT,body,CC_ALWAYS,NULL); getBB()->cfg.attach(&body->cfg,Graph::Edge::BACK);
      }
      --loopDepth; loopTails.pop_back(); setPosition(tail,true);
   }
   void instruction(const std::string &line) {
      auto space=line.find(' '); auto op=opName(line);
      if (op=="KILL") { mkFlow(OP_DISCARD,NULL,CC_ALWAYS,NULL); discard=true; return; }
      if (op=="BRK") {
         require(!loopTails.empty(),"BRK outside loop");
         mkFlow(OP_BREAK,loopTails.back(),CC_ALWAYS,NULL);
         getBB()->cfg.attach(&loopTails.back()->cfg,Graph::Edge::CROSS); return;
      }
      require(space!=std::string::npos,"unsupported opcode: "+line);
      auto a=split(line.substr(space+1),','); require(a.size()>=2,"missing operands");
      Operand d(a[0]); std::vector<unsigned> mask;
      auto sw=d.swizzle.empty() ? std::string("xyzw") : d.swizzle;
      for (char c:sw) mask.push_back(std::string("xyzw").find(c));
      std::array<Value*,4> result{};
      if (op=="TXF") {
         require(a.size()==4 && a[3]=="2D","only integer 2D texel fetch is supported");
         Operand coord(a[1]), sampler(a[2]); require(sampler.file=="SAMP" && sampler.index<3,"invalid sampler");
         samplers |= 1u<<sampler.index;
         std::vector<Value*> src={read(coord,0),read(coord,1),read(coord,3)};
         std::vector<Value*> dst={getSSA(),getSSA(),getSSA(),getSSA()};
         auto *tex=mkTex(OP_TXF,TEX_TARGET_2D,sampler.index,sampler.index,dst,src);
         tex->tex.mask=0xf; tex->tex.levelZero=false;
         for (unsigned c=0;c<4;++c) result[c]=dst[c];
      } else {
         require(a.size()==2 || a.size()==3,"unsupported operand count");
         for (unsigned c:mask) {
            Value *x=read(Operand(a[1]),c), *y=a.size()==3 ? read(Operand(a[2]),c) : NULL;
            Value *r=getSSA();
            if (op=="MOV") mkMov(r,x);
            else if (op=="F2I" || op=="F2U" || op=="U2F") {
               auto *i=mkCvt(OP_CVT,op=="F2I"?TYPE_S32:op=="F2U"?TYPE_U32:TYPE_F32,r,op=="U2F"?TYPE_U32:TYPE_F32,x);
               if (op!="U2F") i->rnd=ROUND_Z;
            } else if (op=="USLT" || op=="USEQ" || op=="USNE") {
               require(y,"missing comparison source");
               mkCmp(OP_SET,op=="USLT"?CC_LT:op=="USEQ"?CC_EQ:CC_NE,TYPE_U32,r,TYPE_U32,x,y);
            } else {
               static const std::map<std::string,operation> ops={{"UADD",OP_ADD},{"UMUL",OP_MUL},{"ADD",OP_ADD},{"MUL",OP_MUL},{"AND",OP_AND},{"OR",OP_OR},{"XOR",OP_XOR},{"SHL",OP_SHL},{"USHR",OP_SHR}};
               require(ops.count(op) && y,"unsupported opcode: "+op);
               mkOp2(ops.at(op),(op=="ADD" || op=="MUL")?TYPE_F32:TYPE_U32,r,x,y);
            }
            result[c]=r;
         }
      }
      // TGSI vectors read their sources before any destination component changes.
      for (unsigned c:mask) write(d,c,result[c]);
   }
   void sequence() {
      while (cursor<lines.size()) {
         auto line=lines[cursor], op=opName(line);
         if (op=="ELSE" || op=="ENDIF" || op=="ENDLOOP" || op=="END") return;
         ++cursor;
         if (op=="UIF") conditional(line);
         else if (op=="BGNLOOP") loop();
         else instruction(line);
      }
   }
public:
   Frontend(Program *p, const std::string &text, bool isVertex):BuildUtil(p),vertex(isVertex) {
      auto *entry=block(); exitBlock=block();
      p->main->setEntry(entry); p->main->setExit(exitBlock); setPosition(entry,true);
      std::istringstream in(text); std::string line;
      while(std::getline(in,line)) { line=trim(line); if(!line.empty()) lines.push_back(line); }
      require(!lines.empty() && lines[0]==(vertex?"VERT":"FRAG"),"invalid shader stage"); cursor=1;
      while(cursor<lines.size() && (lines[cursor].rfind("DCL ",0)==0 || lines[cursor].rfind("IMM[",0)==0)) declaration(lines[cursor++]);
      sequence(); require(cursor<lines.size() && lines[cursor]=="END" && cursor+1==lines.size(),"missing END or trailing input");
      branch(exitBlock); setPosition(exitBlock,true);
      for (auto &[index, values]:outputs) for (unsigned c=0;c<4;++c) if(outputMask[index]&(1u<<c)) {
         Value *v=mkMov(getSSA(),values[c])->getDef(0);
         mkStore(OP_EXPORT,TYPE_U32,mkSymbol(FILE_SHADER_OUTPUT,0,TYPE_U32,outputAddress[index]+c*4),NULL,v);
      }
      mkOp(OP_EXIT,TYPE_NONE,NULL)->terminator=1;
   }
   void metadata(const std::string &prefix, const nv50_ir_prog_info_out &out) {
      uint32_t hdr[20]={};
      hdr[0]=vertex ? 0x20461 : 0x25462;
      if(vertex) hdr[4]=0xff000;
      else { hdr[5]=0x80000000; hdr[18]=0xf; if(discard) hdr[0]|=0x8000; }
      for(auto [i,mask]:inputMask) for(unsigned c=0;c<4;++c) if(mask&(1u<<c)) {
         unsigned a=inputAddress[i]/4+c;
         if(vertex) hdr[5+a/32]|=1u<<(a%32);
         else if(inputAddress[i]==0x70) hdr[5]|=1u<<(24+a-0x60/4);
         else { a*=2; hdr[4+a/32]|=3u<<(a%32); }
      }
      if(vertex) for(auto [i,mask]:outputMask) for(unsigned c=0;c<4;++c) if(mask&(1u<<c)) {
         unsigned a=(outputAddress[i]-0x40)/4+c; hdr[13+a/32]|=1u<<(a%32);
      }
      unsigned tls=ALIGN(getProgram()->tlsSize,16);
      if(tls) { hdr[0]|=1u<<26; hdr[1]=tls; }
      std::ofstream sph(prefix+".sph",std::ios::binary);
      for(auto word:hdr) for(unsigned b=0;b<4;++b) sph.put((word>>(b*8))&255);
      require(bool(sph),"cannot write SPH");
      std::ofstream meta(prefix+".json");
      meta<<"{\n  \"chipset\": 262,\n  \"stage\": \""<<(vertex?"vertex":"fragment")<<"\",\n  \"code_bytes\": "<<getProgram()->binSize<<",\n  \"gprs\": "<<std::max(4,getProgram()->maxGPR+1)<<",\n  \"tls_bytes\": "<<tls<<",\n  \"constant_buffer_slot\": 0,\n  \"constant_buffer_bytes\": "<<cbBytes<<",\n  \"texture_mask\": "<<samplers<<",\n  \"builtin_calls\": 0,\n  \"relocations\": 0,\n  \"fixups_applied\": "<<(out.bin.fixupData?"true":"false")<<",\n  \"input_slots\": [";
      bool first=true;
      for(auto [i,mask]:inputMask) { if(!first) meta<<", "; first=false; meta<<"{\"index\":"<<i<<",\"address\":"<<inputAddress[i]<<",\"mask\":"<<mask<<"}"; }
      meta<<"],\n  \"output_slots\": ["; first=true;
      for(auto [i,mask]:outputMask) { if(!first) meta<<", "; first=false; meta<<"{\"index\":"<<i<<",\"address\":"<<outputAddress[i]<<",\"mask\":"<<mask<<"}"; }
      meta<<"]\n}\n"; require(bool(meta),"cannot write metadata");
   }
};

int main(int argc, char **argv) {
   try {
      require(argc==3,"usage: kepler-compile INPUT.tgsi OUTPUT_PREFIX");
      std::ifstream input(argv[1],std::ios::binary); require(bool(input),"cannot open input");
      std::string text((std::istreambuf_iterator<char>(input)),{});
      bool vertex=text.rfind("VERT",0)==0;
      std::unique_ptr<Target, void(*)(Target*)> targetOwner(Target::create(0x106),Target::destroy);
      Target *target=targetOwner.get(); require(target,"GK208B target unavailable");
      Program prog(vertex?Program::TYPE_VERTEX:Program::TYPE_FRAGMENT,target);
      nv50_ir_prog_info info={}; nv50_ir_prog_info_out out={};
      info.target=0x106; info.type=vertex?PIPE_SHADER_VERTEX:PIPE_SHADER_FRAGMENT; info.optLevel=3;
      info.io.auxCBSlot=15; info.io.texBindBase=0; info.io.suInfoBase=0;
      out.target=0x106; out.type=info.type; out.io.fragDepth=out.io.sampleMask=0xff;
      out.prop.fp.numColourResults=1;
      prog.driver=&info; prog.driver_out=&out; prog.optLevel=3;
      Frontend frontend(&prog,text,vertex);
      target->parseDriverInfo(&info,&out);
      require(target->runLegalizePass(&prog,CG_STAGE_PRE_SSA),"pre-SSA legalization failed");
      require(prog.convertToSSA(),"SSA conversion failed");
      prog.optimizeSSA(3);
      require(target->runLegalizePass(&prog,CG_STAGE_SSA),"SSA legalization failed");
      require(prog.registerAllocation(),"register allocation failed");
      require(target->runLegalizePass(&prog,CG_STAGE_POST_RA),"post-RA legalization failed");
      prog.optimizePostRA(3);
      for (ArrayList::Iterator it=prog.main->allInsns.iterator();!it.end();it.next()) {
         auto *insn=static_cast<Instruction *>(it.get());
         require(insn->op!=OP_CALL,"forbidden compiler helper call");
      }
      require(prog.emitBinary(&out),"GK110 emission failed");
      auto *reloc=static_cast<RelocInfo *>(out.bin.relocData);
      require(!reloc || reloc->count==0,"catalogue emitted relocations");
      // Fixed profile: no multisample, forced flatshade or alpha-test patching.
      if(out.bin.fixupData) nv50_ir_apply_fixups(out.bin.fixupData,prog.code,false,false,0,false);
      require(prog.binSize>0 && prog.binSize%8==0,"invalid GK110 code length");
      std::ofstream code(std::string(argv[2])+".bin",std::ios::binary);
      for(unsigned i=0;i<prog.binSize/4;++i) for(unsigned b=0;b<4;++b) code.put((prog.code[i]>>(b*8))&255);
      require(bool(code),"cannot write code");
      frontend.metadata(argv[2],out);
      // Program owns IR, the emitted malloc buffers belong to its caller.
      free(prog.code); free(out.bin.relocData); free(out.bin.fixupData);
      return 0;
   } catch(const std::exception &e) { fprintf(stderr,"kepler-compile: %s\n",e.what()); return 1; }
}
